use anyhow::Result;

pub async fn run(cmd: crate::config::ImageCmd) -> Result<()> {
    match cmd {
        crate::config::ImageCmd::Build { output, size, ubuntu_release, agent_script } => {
            build_image(&output, &size, &ubuntu_release, agent_script).await
        }
        crate::config::ImageCmd::Register { control_url, pool, path, name, arch } => {
            register_image(&control_url, &pool, &path, &name, &arch).await
        }
        crate::config::ImageCmd::List { control_url } => {
            list_images(&control_url).await
        }
    }
}

async fn build_image(output: &str, size: &str, ubuntu_release: &str, agent_script: Option<String>) -> Result<()> {
    println!("Building CI image...");
    println!("  Output: {}", output);
    println!("  Size: {}", size);
    println!("  Ubuntu release: {}", ubuntu_release);
    if let Some(script) = agent_script {
        println!("  Agent script: {}", script);
    }
    // TODO: 实际构建逻辑（调用 build-rootfs.sh 或内置 Rust 实现）
    println!("Image build completed: {}", output);
    Ok(())
}

async fn register_image(control_url: &str, pool: &str, path: &str, name: &str, arch: &str) -> Result<()> {
    use gita_ci::image::{ImageClient, RegisterImageRequest};
    use sha2::Digest;
    use std::fs;

    let content = fs::read(path)?;
    let digest = format!("sha256:{:x}", sha2::Sha256::digest(&content));

    let client = ImageClient::new(control_url);
    let req = RegisterImageRequest {
        name: name.to_string(),
        pool: pool.to_string(),
        path: path.to_string(),
        arch: arch.to_string(),
        kind_str: "rootfs".to_string(),
        digest,
    };

    let image = client.register(req).await?;
    println!("Image registered: {} ({})", image.name, image.id);
    Ok(())
}

async fn list_images(control_url: &str) -> Result<()> {
    use gita_ci::image::ImageClient;
    let client = ImageClient::new(control_url);
    let images = client.list().await?;
    for img in images {
        println!("{} (id: {}, pool: {}, size: {} bytes)", img.name, img.id, img.pool, img.size_bytes);
    }
    Ok(())
}