//! `mudparty-server [--root <data root>]`: starts the server on a data root (default: the
//! current folder), serves the web page on `listen.client_hub`, runs until Ctrl-C, then logs
//! every bot off.

use std::path::PathBuf;

use anyhow::bail;
use mudparty_server::{Server, web};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from(".");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => match args.next() {
                Some(r) => root = PathBuf::from(r),
                None => bail!("--root needs a folder"),
            },
            "-h" | "--help" => {
                println!("mudparty-server [--root <data root>]");
                return Ok(());
            }
            other => bail!("unknown argument {other:?}; try --help"),
        }
    }
    let server = Server::start(&root).await?;
    let page = web::serve(server.clone(), server.listen()).await?;
    println!(
        "mudparty-server: running {:?}; the page is at http://{page}/; Ctrl-C to stop",
        server.running()
    );
    tokio::signal::ctrl_c().await?;
    for (name, ended) in server.shutdown().await {
        println!("{name}: {ended:?}");
    }
    Ok(())
}
