//! QEMU/KVM 驱动实现
//!
//! 使用 libvirt 或直接调用 qemu-system-x86_64。
//! 这里实现直接调用 qemu-system-x86_64 的简化版本。

use crate::{Error, Result, VmPowerState, VmmDriver};
use async_trait::async_trait;
use gita_vm_api::node::VmmKind;
use gita_vm_api::vm::VmSpec;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct QemuDriver {
    qemu_bin: String,
    work_dir: std::path::PathBuf,
}

impl QemuDriver {
    pub fn new() -> Self {
        Self {
            qemu_bin: "qemu-system-x86_64".into(),
            work_dir: std::path::PathBuf::from("/var/lib/gita/vm"),
        }
    }

    pub fn with_bin(mut self, bin: impl Into<String>) -> Self {
        self.qemu_bin = bin.into();
        self
    }

    pub fn with_work_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.work_dir = dir.into();
        self
    }

    fn vm_dir(&self, vm_id: &str) -> std::path::PathBuf {
        self.work_dir.join(vm_id)
    }

    fn pid_file(&self, vm_id: &str) -> std::path::PathBuf {
        self.vm_dir(vm_id).join("qemu.pid")
    }

    fn socket_path(&self, vm_id: &str) -> std::path::PathBuf {
        self.vm_dir(vm_id).join("qmp.sock")
    }

    fn generate_qemu_cmd(&self, vm_id: &str, spec: &VmSpec) -> Result<Command> {
        let vm_dir = self.vm_dir(vm_id);
        let disk_path = vm_dir.join("disk.qcow2");
        let pid_file = self.pid_file(vm_id);
        let qmp_socket = self.socket_path(vm_id);

        let mut cmd = Command::new(&self.qemu_bin);
        cmd.arg("-name").arg(vm_id)
            .arg("-pidfile").arg(&pid_file)
            .arg("-daemonize")
            .arg("-enable-kvm")
            .arg("-cpu").arg("host")
            .arg("-smp").arg(format!("{},cores={},threads=1",
                spec.cpu_millis / 1000,
                (spec.cpu_millis / 1000).min(8)))
            .arg("-m").arg(format!("{}M", spec.memory_bytes / 1024 / 1024))
            .arg("-drive").arg(format!("file={},format=qcow2,if=virtio,cache=writeback", disk_path.display()))
            .arg("-netdev").arg(format!("tap,id=net0,ifname=tap-{vm_id},script=no,downscript=no"))
            .arg("-device").arg("virtio-net-pci,netdev=net0")
            .arg("-qmp").arg(format!("unix:{},server,nowait", qmp_socket.display()))
            .arg("-display").arg("none")
            .arg("-serial").arg("stdio")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // 用户数据通过 cloud-init ISO 注入
        if spec.user_data.is_some() {
            let ci_iso = vm_dir.join("cidata.iso");
            if ci_iso.exists() {
                cmd.arg("-drive").arg(format!("file={},format=raw,if=virtio,readonly=on", ci_iso.display()));
            }
        }

        Ok(cmd)
    }
}

#[async_trait]
impl VmmDriver for QemuDriver {
    fn kind(&self) -> VmmKind {
        VmmKind::Qemu
    }

    async fn define_vm(&self, vm_id: &str, spec: &VmSpec, work_dir: &Path) -> Result<()> {
        let vm_dir = work_dir.join(vm_id);
        tokio::fs::create_dir_all(&vm_dir).await.map_err(|e| Error::Io(e))?;

        // 创建磁盘镜像（从基础镜像 copy-on-write）
        let disk_path = vm_dir.join("disk.qcow2");
        let _base_image = &spec.image.name;

        // 这里简化：实际应该从镜像仓库拉取 base image
        // 暂时创建一个空磁盘
        let output = Command::new("qemu-img")
            .args(["create", "-f", "qcow2", &disk_path.to_string_lossy(), &format!("{}", spec.disk_bytes)])
            .output()
            .await
            .map_err(|e| Error::ProcessError(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::OperationFailed(
                String::from_utf8_lossy(&output.stderr).into()
            ));
        }

        // 生成 cloud-init ISO
        if let Some(user_data) = &spec.user_data {
            let ci_iso = vm_dir.join("cidata.iso");
            let meta_data = format!("instance-id: {vm_id}\nlocal-hostname: {vm_id}\n");
            let user_data_file = vm_dir.join("user-data");
            let meta_data_file = vm_dir.join("meta-data");

            tokio::fs::write(&user_data_file, user_data).await.map_err(|e| Error::Io(e))?;
            tokio::fs::write(&meta_data_file, meta_data).await.map_err(|e| Error::Io(e))?;

            let _ = Command::new("cloud-localds")
                .args([ci_iso.to_string_lossy().as_ref(), user_data_file.to_string_lossy().as_ref(), meta_data_file.to_string_lossy().as_ref()])
                .output()
                .await
                .map_err(|e| Error::ProcessError(e.to_string()))?;

            if !output.status.success() {
                tracing::warn!("cloud-localds failed: {}", String::from_utf8_lossy(&output.stderr));
            }
        }

        Ok(())
    }

    async fn start_vm(&self, vm_id: &str) -> Result<()> {
        // VM 应该已经通过 define_vm 创建好磁盘和配置
        // 这里直接启动（如果未 daemonize 则需要管理进程）
        let vm_dir = self.vm_dir(vm_id);
        let pid_file = self.pid_file(vm_id);

        // 检查是否已运行
        if pid_file.exists() {
            if let Ok(pid_str) = tokio::fs::read_to_string(&pid_file).await {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    // 检查进程是否存在
                    if tokio::process::Command::new("kill")
                        .args(["-0", &pid.to_string()])
                        .status()
                        .await
                        .map(|s| s.success())
                        .unwrap_or(false)
                    {
                        return Ok(()); // 已经在运行
                    }
                }
            }
        }

        // 这里需要 spec 信息，实际应该从持久化存储读取
        // 简化：假设已有配置文件
        let spec_file = vm_dir.join("spec.json");
        if !spec_file.exists() {
            return Err(Error::NotFound(format!("VM {vm_id} spec not found")));
        }

        let spec_json = tokio::fs::read_to_string(&spec_file).await.map_err(|e| Error::Io(e))?;
        let spec: VmSpec = serde_json::from_str(&spec_json).map_err(|e| Error::ConfigGeneration(e.to_string()))?;

        let mut cmd = self.generate_qemu_cmd(vm_id, &spec)?;
        let _output = cmd.spawn().map_err(|e| Error::ProcessError(e.to_string()))?;

        // 等待启动确认
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        Ok(())
    }

    async fn stop_vm(&self, vm_id: &str, force: bool) -> Result<()> {
        let pid_file = self.pid_file(vm_id);
        if !pid_file.exists() {
            return Ok(()); // 已经停止
        }

        let pid_str = tokio::fs::read_to_string(&pid_file).await.map_err(|e| Error::Io(e))?;
        let pid = pid_str.trim().parse::<u32>().map_err(|e| Error::OperationFailed(e.to_string()))?;

        let signal = if force { "KILL" } else { "TERM" };
        let status = Command::new("kill")
            .args([format!("-{signal}"), pid.to_string()])
            .status()
            .await
            .map_err(|e| Error::ProcessError(e.to_string()))?;

        if force || status.success() {
            // 等待进程退出
            for _ in 0..30 {
                if Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .status()
                    .await
                    .map(|s| !s.success())
                    .unwrap_or(true)
                {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            let _ = tokio::fs::remove_file(&pid_file).await;
        }

        Ok(())
    }

    async fn delete_vm(&self, vm_id: &str) -> Result<()> {
        let _ = self.stop_vm(vm_id, true).await;
        let vm_dir = self.vm_dir(vm_id);
        tokio::fs::remove_dir_all(&vm_dir).await.map_err(|e| Error::Io(e))?;
        Ok(())
    }

    async fn get_vm_status(&self, vm_id: &str) -> Result<VmPowerState> {
        let pid_file = self.pid_file(vm_id);
        if !pid_file.exists() {
            return Ok(VmPowerState::Shutoff);
        }

        let pid_str = tokio::fs::read_to_string(&pid_file).await.map_err(|e| Error::Io(e))?;
        let pid = pid_str.trim().parse::<u32>().map_err(|e| Error::OperationFailed(e.to_string()))?;

        let running = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if running {
            Ok(VmPowerState::Running)
        } else {
            Ok(VmPowerState::Shutoff)
        }
    }

    async fn get_console(&self, vm_id: &str) -> Result<Option<String>> {
        // QMP socket 或 VNC 端口
        let qmp = self.socket_path(vm_id);
        if qmp.exists() {
            return Ok(Some(format!("qmp+unix://{}", qmp.display())));
        }
        Ok(None)
    }

    async fn hotplug(&self, _vm_id: &str, _cpu_millis: Option<u64>, _memory_bytes: Option<u64>) -> Result<()> {
        // 通过 QMP 热插 CPU/内存
        // 简化：返回未实现
        Err(Error::OperationFailed("hotplug not implemented".into()))
    }

    async fn resize_disk(&self, vm_id: &str, new_size_bytes: u64) -> Result<()> {
        let disk_path = self.vm_dir(vm_id).join("disk.qcow2");
        let output = Command::new("qemu-img")
            .args(["resize", &disk_path.to_string_lossy(), &format!("{}", new_size_bytes)])
            .output()
            .await
            .map_err(|e| Error::ProcessError(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::OperationFailed(
                String::from_utf8_lossy(&output.stderr).into()
            ));
        }
        Ok(())
    }

    async fn create_snapshot(&self, vm_id: &str, snapshot_path: &Path) -> Result<()> {
        let disk_path = self.vm_dir(vm_id).join("disk.qcow2");
        let output = Command::new("qemu-img")
            .args(["snapshot", "-c", &snapshot_path.file_name().unwrap().to_string_lossy(), &disk_path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| Error::ProcessError(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::OperationFailed(
                String::from_utf8_lossy(&output.stderr).into()
            ));
        }
        Ok(())
    }

    async fn restore_snapshot(&self, vm_id: &str, snapshot_path: &Path) -> Result<()> {
        let disk_path = self.vm_dir(vm_id).join("disk.qcow2");
        let output = Command::new("qemu-img")
            .args(["snapshot", "-a", &snapshot_path.file_name().unwrap().to_string_lossy(), &disk_path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| Error::ProcessError(e.to_string()))?;

        if !output.status.success() {
            return Err(Error::OperationFailed(
                String::from_utf8_lossy(&output.stderr).into()
            ));
        }
        Ok(())
    }
}

impl Default for QemuDriver {
    fn default() -> Self {
        Self::new()
    }
}