use std::io;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    wechat_devify_bridge_lib::mcp::run_from_env().await.map_err(io::Error::other)?;
    Ok(())
}
