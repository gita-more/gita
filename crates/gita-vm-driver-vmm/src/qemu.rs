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
    /// The disk tool. Injectable so that the error paths can be
    /// exercised without a working QEMU installation.
    qemu_img_bin: String,
    /// The cloud-init image builder.
    cloud_localds_bin: String,
    work_dir: std::path::PathBuf,
}

impl QemuDriver {
    pub fn new() -> Self {
        Self {
            qemu_bin: "qemu-system-x86_64".into(),
            qemu_img_bin: "qemu-img".into(),
            cloud_localds_bin: "cloud-localds".into(),
            work_dir: std::path::PathBuf::from("/var/lib/gita/vm"),
        }
    }

    pub fn with_bin(mut self, bin: impl Into<String>) -> Self {
        self.qemu_bin = bin.into();
        self
    }

    /// The disk tool to use instead of `qemu-img`.
    pub fn with_qemu_img_bin(mut self, bin: impl Into<String>) -> Self {
        self.qemu_img_bin = bin.into();
        self
    }

    /// The cloud-init image builder to use instead of `cloud-localds`.
    pub fn with_cloud_localds_bin(mut self, bin: impl Into<String>) -> Self {
        self.cloud_localds_bin = bin.into();
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
        let output = Command::new(&self.qemu_img_bin)
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

            let ci_output = Command::new(&self.cloud_localds_bin)
                .args([ci_iso.to_string_lossy().as_ref(), user_data_file.to_string_lossy().as_ref(), meta_data_file.to_string_lossy().as_ref()])
                .output()
                .await
                .map_err(|e| Error::ProcessError(e.to_string()))?;

            // The status to look at is the one of the command that just
            // ran: the disk image was built by an earlier command, and
            // its status says nothing about this one.
            if !ci_output.status.success() {
                return Err(Error::OperationFailed(
                    String::from_utf8_lossy(&ci_output.stderr).into()
                ));
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
        let output = Command::new(&self.qemu_img_bin)
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
        let Some(snapshot_name) = snapshot_path.file_name() else {
            return Err(Error::OperationFailed(format!(
                "snapshot path has no file name: {}",
                snapshot_path.display()
            )));
        };
        let output = Command::new(&self.qemu_img_bin)
            .args(["snapshot", "-c", &snapshot_name.to_string_lossy(), &disk_path.to_string_lossy()])
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
        let Some(snapshot_name) = snapshot_path.file_name() else {
            return Err(Error::OperationFailed(format!(
                "snapshot path has no file name: {}",
                snapshot_path.display()
            )));
        };
        let output = Command::new(&self.qemu_img_bin)
            .args(["snapshot", "-a", &snapshot_name.to_string_lossy(), &disk_path.to_string_lossy()])
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

#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::vm::VmSpec;

    fn driver(dir: &Path) -> QemuDriver {
        QemuDriver::new().with_work_dir(dir.to_path_buf())
    }

    fn spec() -> VmSpec {
        VmSpec {
            cpu_millis: 4000,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            ..Default::default()
        }
    }

    /// The arguments of the command that starts the VM. The driver
    /// builds it without running it, so the arguments can be read.
    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    /// The value of an option, and the one after it.
    fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        let idx = args.iter().position(|a| a == name)?;
        args.get(idx + 1).map(String::as_str)
    }

    /// The VM's files live in a directory named after the VM.
    #[test]
    fn the_vm_files_live_under_the_vm_directory() {
        let dir = Path::new("/var/lib/gita/vm");
        let d = driver(dir);
        assert_eq!(d.vm_dir("vm-1"), dir.join("vm-1"));
        assert_eq!(d.pid_file("vm-1"), dir.join("vm-1").join("qemu.pid"));
        assert_eq!(d.socket_path("vm-1"), dir.join("vm-1").join("qmp.sock"));
    }

    /// The command names the VM, points at its disk, and gives it the
    /// resources the spec asks for.
    #[test]
    fn the_command_carries_the_vm_and_its_resources() {
        let dir = Path::new("/var/lib/gita/vm");
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &spec()).unwrap();
        let args = args_of(&cmd);

        assert_eq!(option(&args, "-name"), Some("vm-1"));
        assert_eq!(
            option(&args, "-pidfile"),
            Some("/var/lib/gita/vm/vm-1/qemu.pid")
        );
        assert_eq!(
            option(&args, "-drive"),
            Some("file=/var/lib/gita/vm/vm-1/disk.qcow2,format=qcow2,if=virtio,cache=writeback")
        );
        assert_eq!(
            option(&args, "-qmp"),
            Some("unix:/var/lib/gita/vm/vm-1/qmp.sock,server,nowait")
        );
        assert_eq!(option(&args, "-m"), Some("2048M"));
    }

    /// The CPU count is the number of cores the spec asks for, capped
    /// at eight.
    #[test]
    fn the_cpu_count_is_capped_at_eight_cores() {
        let dir = Path::new("/var/lib/gita/vm");
        let mut big = spec();
        big.cpu_millis = 64_000;
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &big).unwrap();
        let args = args_of(&cmd);
        let smp = option(&args, "-smp").expect("the smp option");
        assert!(smp.starts_with("64,cores=8"), "got {smp}");

        let mut small = spec();
        small.cpu_millis = 1500;
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &small).unwrap();
        let args = args_of(&cmd);
        let smp = option(&args, "-smp").expect("the smp option");
        assert!(smp.starts_with("1,cores=1"), "got {smp}");
    }

    /// A memory size below a megabyte rounds down to nothing, and the
    /// command then asks for zero megabytes.
    #[test]
    fn the_memory_is_given_in_megabytes() {
        let dir = Path::new("/var/lib/gita/vm");
        let mut tiny = spec();
        tiny.memory_bytes = 512 * 1024;
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &tiny).unwrap();
        let args = args_of(&cmd);
        assert_eq!(option(&args, "-m"), Some("0M"));
    }

    /// The cloud-init disk is only attached when there is user data
    /// *and* the image was built.
    #[test]
    fn the_cloud_init_disk_is_only_attached_when_it_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();

        let mut with_data = spec();
        with_data.user_data = Some("instance: yes\n".to_string());
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &with_data).unwrap();
        let args = args_of(&cmd);
        assert!(
            !args.iter().any(|a| a.contains("cidata.iso")),
            "the drive must not be attached before the image exists"
        );

        // Build the image the way define_vm would.
        std::fs::create_dir_all(dir.join("vm-1")).unwrap();
        std::fs::write(dir.join("vm-1").join("cidata.iso"), b"iso").unwrap();
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &with_data).unwrap();
        let args = args_of(&cmd);
        assert!(
            args.iter().any(|a| a.contains("cidata.iso")),
            "the drive must be attached once the image exists"
        );

        // Without user data the image is not attached even if it is there.
        let cmd = driver(dir).generate_qemu_cmd("vm-1", &spec()).unwrap();
        let args = args_of(&cmd);
        assert!(!args.iter().any(|a| a.contains("cidata.iso")));
    }

    /// Defining a VM creates its directory and its disk.
    #[tokio::test]
    async fn defining_a_vm_creates_its_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        d.define_vm("vm-1", &spec(), tmp.path()).await.unwrap();

        let disk = tmp.path().join("vm-1").join("disk.qcow2");
        assert!(disk.exists(), "the disk must be created");
    }

    /// A disk tool that fails makes the definition fail.
    #[tokio::test]
    async fn a_failing_disk_tool_fails_the_definition() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path()).with_qemu_img_bin("/bin/false");
        let err = d
            .define_vm("vm-1", &spec(), tmp.path())
            .await
            .expect_err("the definition must fail");
        assert!(
            matches!(err, Error::OperationFailed(_)),
            "got {err:?}"
        );
    }

    /// A disk tool that is not there at all is a process error.
    #[tokio::test]
    async fn a_missing_disk_tool_is_a_process_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path()).with_qemu_img_bin("/nonexistent/qemu-img");
        let err = d
            .define_vm("vm-1", &spec(), tmp.path())
            .await
            .expect_err("the definition must fail");
        assert!(matches!(err, Error::ProcessError(_)), "got {err:?}");
    }

    /// A cloud-init builder that fails must fail the definition: the VM
    /// would otherwise start without the user data it was given.
    #[tokio::test]
    async fn a_failing_cloud_init_builder_fails_the_definition() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut with_data = spec();
        with_data.user_data = Some("instance: yes\n".to_string());
        let d = driver(tmp.path()).with_cloud_localds_bin("/bin/false");
        let err = d
            .define_vm("vm-1", &with_data, tmp.path())
            .await
            .expect_err("the definition must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// A VM that was never defined cannot be started.
    #[tokio::test]
    async fn starting_a_vm_that_was_never_defined_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        let err = d.start_vm("vm-1").await.expect_err("the start must fail");
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    /// A VM whose spec file cannot be parsed cannot be started either.
    #[tokio::test]
    async fn starting_a_vm_with_a_broken_spec_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        std::fs::create_dir_all(tmp.path().join("vm-1")).unwrap();
        std::fs::write(tmp.path().join("vm-1").join("spec.json"), b"{not json").unwrap();
        let err = d.start_vm("vm-1").await.expect_err("the start must fail");
        assert!(matches!(err, Error::ConfigGeneration(_)), "got {err:?}");
    }

    /// Stopping a VM that is not running succeeds: the caller asked for
    /// it to be stopped, and it is.
    #[tokio::test]
    async fn stopping_a_vm_that_is_not_running_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        d.stop_vm("vm-1", false).await.unwrap();
        d.stop_vm("vm-1", true).await.unwrap();
    }

    /// A pid file that does not hold a pid cannot be acted on.
    #[tokio::test]
    async fn a_pid_file_that_is_not_a_pid_is_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        std::fs::create_dir_all(tmp.path().join("vm-1")).unwrap();
        std::fs::write(tmp.path().join("vm-1").join("qemu.pid"), b"not a pid").unwrap();

        let err = d.stop_vm("vm-1", false).await.expect_err("the stop must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");

        let err = d.get_vm_status("vm-1").await.expect_err("the status must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// A VM with no pid file is off.
    #[tokio::test]
    async fn a_vm_with_no_pid_file_is_off() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        assert_eq!(d.get_vm_status("vm-1").await.unwrap(), VmPowerState::Shutoff);
    }

    /// A VM whose pid is alive is running.
    #[tokio::test]
    async fn a_vm_whose_pid_is_alive_is_running() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        std::fs::create_dir_all(tmp.path().join("vm-1")).unwrap();
        // The pid of this test process is alive by definition.
        let pid = std::process::id();
        std::fs::write(
            tmp.path().join("vm-1").join("qemu.pid"),
            pid.to_string(),
        )
        .unwrap();
        assert_eq!(d.get_vm_status("vm-1").await.unwrap(), VmPowerState::Running);
    }

    /// The console is the QMP socket, and only if it is there.
    #[tokio::test]
    async fn the_console_is_the_qmp_socket() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        assert_eq!(d.get_console("vm-1").await.unwrap(), None);

        std::fs::create_dir_all(tmp.path().join("vm-1")).unwrap();
        std::fs::write(tmp.path().join("vm-1").join("qmp.sock"), b"").unwrap();
        let console = d.get_console("vm-1").await.unwrap().expect("the console");
        assert!(console.contains("qmp+unix://"), "got {console}");
    }

    /// Hotplugging is not implemented; saying so is better than
    /// pretending it worked.
    #[tokio::test]
    async fn hotplug_is_not_implemented() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        let err = d
            .hotplug("vm-1", Some(1000), None)
            .await
            .expect_err("the hotplug must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// A disk tool that fails makes the resize fail.
    #[tokio::test]
    async fn a_failing_disk_tool_fails_the_resize() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path()).with_qemu_img_bin("/bin/false");
        let err = d
            .resize_disk("vm-1", 1024)
            .await
            .expect_err("the resize must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// A snapshot path with no file name in it cannot name a snapshot.
    #[tokio::test]
    async fn a_snapshot_path_without_a_file_name_is_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path()).with_qemu_img_bin("/bin/false");
        let err = d
            .create_snapshot("vm-1", Path::new("/"))
            .await
            .expect_err("the snapshot must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");

        let err = d
            .restore_snapshot("vm-1", Path::new("/"))
            .await
            .expect_err("the restore must fail");
        assert!(matches!(err, Error::OperationFailed(_)), "got {err:?}");
    }

    /// Deleting a VM removes its directory.
    #[tokio::test]
    async fn deleting_a_vm_removes_its_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let d = driver(tmp.path());
        d.define_vm("vm-1", &spec(), tmp.path()).await.unwrap();
        assert!(tmp.path().join("vm-1").exists());

        d.delete_vm("vm-1").await.unwrap();
        assert!(!tmp.path().join("vm-1").exists());
    }
}
