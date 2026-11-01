use anyhow::Result;

pub async fn run(cmd: crate::config::TemplateCmd) -> Result<()> {
    match cmd {
        crate::config::TemplateCmd::Create {
            control_url,
            api_key,
            image,
            kernel,
            template,
            cpu,
            memory,
            disk,
            network,
        } => {
            create_template(
                &control_url,
                api_key.as_deref(),
                &image,
                kernel.as_deref(),
                &template,
                cpu,
                &memory,
                &disk,
                &network,
            )
            .await
        }
        crate::config::TemplateCmd::List {
            control_url,
            api_key,
        } => list_templates(&control_url, api_key.as_deref()).await,
    }
}

async fn create_template(
    control_url: &str,
    api_key: Option<&str>,
    image: &str,
    kernel: Option<&str>,
    template: &str,
    cpu: u64,
    memory: &str,
    disk: &str,
    network: &str,
) -> Result<()> {
    use crate::config::parse_size;
    use gita_ci::template::{
        CreateTemplateRequest, ImageRef, NetworkAttachment, VmSpec,
    };

    let client = gita_ci::TemplateClient::with_key(control_url, api_key);
    let req = CreateTemplateRequest {
        name: template.to_string(),
        spec: VmSpec {
            cpu_millis: cpu,
            memory_bytes: parse_size(memory)?,
            disk_bytes: parse_size(disk)?,
            image: ImageRef {
                name: image.to_string(),
                digest: None,
            },
            kernel: kernel.map(|k| ImageRef {
                name: k.to_string(),
                digest: None,
            }),
            networks: vec![NetworkAttachment {
                network: network.to_string(),
                ip: None,
                mac: None,
            }],
            backend: "qemu".to_string(),
            arch: None,
            user_data: None,
        },
    };

    let template = client.create(req).await?;
    println!(
        "Template created: {} (id: {})",
        template.name, template.id
    );
    Ok(())
}

async fn list_templates(control_url: &str, api_key: Option<&str>) -> Result<()> {
    let client = gita_ci::TemplateClient::with_key(control_url, api_key);
    let templates = client.list().await?;
    for tmpl in templates {
        println!("{} (id: {})", tmpl.name, tmpl.id);
    }
    Ok(())
}