//! 节点代理配置

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "gita-vm-node", version, about = "Gita VM node agent")]
pub struct NodeConfig {
    /// 控制面 URL（gita-server 或独立 gita-vm-control）
    #[arg(long, env = "GITA_CONTROL_URL", default_value = "http://127.0.0.1:3000")]
    pub control_url: String,

    /// 节点名（默认取主机名）
    #[arg(long, env = "GITA_NODE_NAME")]
    pub node_name: Option<String>,

    /// API key（控制面内部接口认证）。部署未显式配置时用
    /// `gita-admin vm api-key` 从控制面读出生成的值。
    #[arg(long, default_value = "")]
    pub api_key: String,

    /// 工作目录（VM 磁盘、配置存放）
    #[arg(long, default_value = "/var/lib/gita/vm")]
    pub work_dir: String,

    /// 本机总 CPU（毫核）
    #[arg(long, default_value_t = default_cpu_millis())]
    pub cpu_millis: u64,

    /// 本机总内存（字节）
    #[arg(long, default_value_t = default_memory_bytes())]
    pub memory_bytes: u64,

    /// 本机可用磁盘（字节）
    #[arg(long, default_value_t = default_disk_bytes())]
    pub disk_bytes: u64,

    /// 支持的 VMM（逗号分隔）
    #[arg(long, default_value = "qemu")]
    pub vmm: String,

    /// 轮询间隔（秒）
    #[arg(long, default_value_t = 10)]
    pub poll_interval_secs: u64,
}

fn default_cpu_millis() -> u64 {
    // 读取 /proc/cpuinfo 核心数 × 1000
    8000
}

fn default_memory_bytes() -> u64 {
    16_000_000_000
}

fn default_disk_bytes() -> u64 {
    100_000_000_000
}

impl NodeConfig {
    pub fn node_name_or_hostname(&self) -> String {
        self.node_name
            .clone()
            .unwrap_or_else(|| {
                hostname().unwrap_or_else(|_| "unknown-node".to_string())
            })
    }

    pub fn vmm_kinds(&self) -> Vec<gita_vm_api::node::VmmKind> {
        self.vmm
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    }
}

/// The host's name, or an error if the platform has none.
///
/// `gethostname` fails with a negative return value; the bytes after the
/// name are whatever was on the stack, so they must not be read.
fn hostname() -> std::io::Result<String> {
    let mut buf = [0u8; 256];
    let len = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if len < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // glibc returns 0 and leaves the name NUL-terminated in the buffer;
    // the raw syscall returns the length of the name without the
    // terminator. Either way the name ends at the first NUL.
    let limit = if len > 0 { (len as usize).min(buf.len()) } else { buf.len() };
    Ok(hostname_from(&buf[..limit]))
}

/// The name in the bytes `gethostname` reported.
fn hostname_from(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vmm: &str) -> NodeConfig {
        NodeConfig {
            control_url: "http://127.0.0.1:3000".to_string(),
            node_name: None,
            api_key: String::new(),
            work_dir: "/var/lib/gita/vm".to_string(),
            cpu_millis: 8000,
            memory_bytes: 16_000_000_000,
            disk_bytes: 100_000_000_000,
            vmm: vmm.to_string(),
            poll_interval_secs: 10,
        }
    }

    /// The VMM list is comma separated; whitespace around a name is
    /// ignored and an unknown name is dropped rather than fatal.
    #[test]
    fn the_vmm_list_is_split_and_filtered() {
        let kinds = config("qemu").vmm_kinds();
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].as_str(), "qemu");

        let kinds = config(" qemu , foo ,, qemu ").vmm_kinds();
        assert_eq!(kinds.len(), 2, "got {kinds:?}");
        assert!(kinds.iter().all(|k| k.as_str() == "qemu"));
    }

    /// A list with no supported name yields no VMM at all — the node
    /// advertises no backend.
    #[test]
    fn an_empty_or_unknown_vmm_list_yields_nothing() {
        assert!(config("").vmm_kinds().is_empty());
        assert!(config("firecracker").vmm_kinds().is_empty());
    }

    /// The name is matched case-insensitively.
    #[test]
    fn the_vmm_name_is_matched_case_insensitively() {
        assert_eq!(config("QEMU").vmm_kinds().len(), 1);
    }

    /// A configured name wins over the hostname.
    #[test]
    fn a_configured_node_name_is_used() {
        let mut cfg = config("qemu");
        cfg.node_name = Some("node-1".to_string());
        assert_eq!(cfg.node_name_or_hostname(), "node-1");
    }

    /// Without a configured name the hostname is used, and it must be
    /// exactly the hostname — not the hostname followed by whatever was
    /// on the stack.
    #[test]
    fn the_hostname_is_used_when_no_name_is_configured() {
        let name = config("qemu").node_name_or_hostname();
        assert!(!name.is_empty(), "the hostname must not be empty");
        let expected = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .expect("read the hostname")
            .trim()
            .to_string();
        assert_eq!(name, expected, "the hostname must be exact");
    }

    /// Reading the hostname must not fail: the fallback is only for a
    /// machine that has no hostname at all.
    #[test]
    fn reading_the_hostname_succeeds() {
        assert!(hostname().is_ok());
    }

    /// Only the bytes the syscall reported are the name; the rest of
    /// the buffer is stack memory and must not leak into it.
    #[test]
    fn the_name_stops_where_the_syscall_says() {
        assert_eq!(hostname_from(b"node-1"), "node-1");
        assert_eq!(hostname_from(b"node-1\0"), "node-1");
        assert_eq!(hostname_from(b""), "");
    }

    /// A name that is terminated inside the reported bytes stops at the
    /// terminator, whatever follows it.
    #[test]
    fn a_terminated_name_stops_at_the_terminator() {
        assert_eq!(hostname_from(b"node-1\0leftover"), "node-1");
    }
}
