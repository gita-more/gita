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

    /// API key（控制面内部接口认证）
    #[arg(long, env = "GITA_VM_API_KEY", default_value = "")]
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

fn hostname() -> std::io::Result<String> {
    let mut buf = [0u8; 256];
    let len = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if len != 0 {
        return Ok(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string());
    }
    Ok(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string())
}
