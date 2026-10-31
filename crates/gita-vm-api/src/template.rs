//! 模板相关类型

use crate::ids::{ImageId, TemplateId};
use crate::vm::VmSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 模板规格（用于创建/更新请求）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TemplateSpec {
    /// 模板名
    pub name: String,
    /// 期望规格
    pub spec: VmSpec,
    /// 可选：模板对应的镜像 ID
    #[serde(default)]
    pub image_id: Option<ImageId>,
    /// 可选：模板对应的内核镜像 ID
    #[serde(default)]
    pub kernel_image_id: Option<ImageId>,
}

/// 虚机模板：版本化的期望定义，用于批量实例化
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VmTemplate {
    pub id: TemplateId,
    pub name: String,
    /// 模板版本号，内容变更时 +1
    pub version: u64,
    pub spec: VmSpec,
    pub created_at_ms: i64,
    /// 可选：模板对应的镜像 ID
    #[serde(default)]
    pub image_id: Option<ImageId>,
    /// 可选：模板对应的内核镜像 ID
    #[serde(default)]
    pub kernel_image_id: Option<ImageId>,
    /// 资源归属
    pub owner: crate::ids::OwnerRef,
    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 描述
    #[serde(default)]
    pub description: String,
}

impl VmTemplate {
    /// 从模板实例化 VM Spec（允许运行时覆盖）
    pub fn instantiate(&self, overrides: Option<&VmSpecOverrides>) -> VmSpec {
        let mut spec = self.spec.clone();
        if let Some(o) = overrides {
            if let Some(cpu) = o.cpu_millis { spec.cpu_millis = cpu; }
            if let Some(mem) = o.memory_bytes { spec.memory_bytes = mem; }
            if let Some(disk) = o.disk_bytes { spec.disk_bytes = disk; }
            if let Some(ref img) = o.image { spec.image = img.clone(); }
            if let Some(ref kernel) = o.kernel { spec.kernel = Some(kernel.clone()); }
            if let Some(ref nets) = o.networks { spec.networks = nets.clone(); }
            if let Some(ref ud) = o.user_data { spec.user_data = Some(ud.clone()); }
            if let Some(ref meta) = o.metadata { spec.metadata = meta.clone(); }
        }
        spec
    }
}

/// 实例化时的覆盖参数
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VmSpecOverrides {
    #[serde(default)]
    pub cpu_millis: Option<u64>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub disk_bytes: Option<u64>,
    #[serde(default)]
    pub image: Option<crate::vm::ImageRef>,
    #[serde(default)]
    pub kernel: Option<crate::vm::ImageRef>,
    #[serde(default)]
    pub networks: Option<Vec<crate::network::NetworkAttachment>>,
    #[serde(default)]
    pub user_data: Option<String>,
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{ImageRef, VmSpec};

    #[test]
    fn template_instantiate_with_overrides() {
        let tmpl = VmTemplate {
            id: TemplateId(1),
            name: "ci-runner".into(),
            version: 1,
            spec: VmSpec {
                cpu_millis: 2000,
                memory_bytes: 2_000_000_000,
                disk_bytes: 10_000_000_000,
                image: ImageRef { name: "ubuntu-24.04".into(), digest: None },
                kernel: None,
                networks: vec![],
                backend: crate::node::VmmKind::Qemu,
                arch: None,
                user_data: None,
                metadata: BTreeMap::new(),
                template: None,
                affinity: None,
                tolerations: vec![],
                priority: 0,
            },
            created_at_ms: 0,
            image_id: None,
            kernel_image_id: None,
            owner: crate::ids::OwnerRef::system(),
            tags: vec![],
            description: "".into(),
        };

        let overrides = VmSpecOverrides {
            cpu_millis: Some(4000),
            memory_bytes: Some(4_000_000_000),
            ..Default::default()
        };
        let spec = tmpl.instantiate(Some(&overrides));
        assert_eq!(spec.cpu_millis, 4000);
        assert_eq!(spec.memory_bytes, 4_000_000_000);
        // 未覆盖的保留模板值
        assert_eq!(spec.disk_bytes, 10_000_000_000);
    }
}