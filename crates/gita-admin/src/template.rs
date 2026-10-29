use anyhow::Result;

pub async fn run(cmd: crate::config::TemplateCmd) -> Result<()> {
    match cmd {
        crate::config::TemplateCmd::Create { control_url, image, template, cpu, memory, disk, network } => {
            create_template(&control_url, &image, &template, cpu, &memory, &disk, &network).await
        }
        crate::config::TemplateCmd::List { control_url } => {
            list_templates(&control_url).await
        }
    }
}

async fn create_template(control_url: &str, image: &str, template: &str, cpu: u64, memory: &str, disk: &str, network: &str) -> Result<()> {
    use crate::config::parse_size;

    let client = gita_ci::TemplateClient::new(control_url);
    let req = gita_ci::template::CreateTemplateRequest {
        name: template.to_string(),
        image: image.to_string(),
        cpu_millis: cpu,
        memory_bytes: parse_size(memory)?,
        disk_bytes: parse_size(disk)?,
        network: network.to_string(),
    };

    let template = client.create(req).await?;
    println!("Template created: {} (id: {})", template.name, template.id);
    Ok(())
}

async fn list_templates(control_url: &str) -> Result<()> {
    let client = gita_ci::TemplateClient::new(control_url);
    let templates = client.list().await?;
    for tmpl in templates {
        println!("{} (id: {})", tmpl.name, tmpl.id);
    }
    Ok(())
}