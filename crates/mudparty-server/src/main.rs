//! `mudparty-server [--root <data root>]`: starts the server on a data root (default: the
//! current folder), serves the web page on `listen.client_hub` and the MudPlay proxy on
//! `listen.proxy`, runs until Ctrl-C, then logs every bot off.
//!
//! `mudparty-server places compile [--area <name>] [--root <data root>]`: writes the
//! compiled room files into `places/` (design §14.4) and exits.

use std::path::PathBuf;

use anyhow::bail;
use mudparty_server::{Server, compile_places, proxy, web};

const USAGE: &str = "mudparty-server [--root <data root>]
mudparty-server places compile [--area <name>] [--root <data root>]";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from(".");
    let mut words: Vec<String> = Vec::new();
    let mut area: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => match args.next() {
                Some(r) => root = PathBuf::from(r),
                None => bail!("--root needs a folder"),
            },
            "--area" => match args.next() {
                Some(n) => area = Some(n),
                None => bail!("--area needs an area's name"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            w if !w.starts_with('-') => words.push(w.to_owned()),
            other => bail!("unknown argument {other:?}; try --help"),
        }
    }
    match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] if area.is_none() => {}
        ["places", "compile"] => {
            let written = compile_places(&root, area.as_deref())?;
            let rooms: usize = written.iter().map(|(_, n)| n).sum();
            println!(
                "mudparty-server: wrote {} areas ({rooms} rooms) to {}",
                written.len(),
                root.join("places").display()
            );
            return Ok(());
        }
        _ => bail!("unknown command; try --help"),
    }
    let server = Server::start(&root).await?;
    let page = web::serve(server.clone(), server.listen()).await?;
    let seats = proxy::serve(server.clone(), server.proxy_listen()).await?;
    println!(
        "mudparty-server: running {:?}; the page is at http://{page}/; MudPlay connects to {seats}; Ctrl-C to stop",
        server.running()
    );
    tokio::signal::ctrl_c().await?;
    for (name, ended) in server.shutdown().await {
        println!("{name}: {ended:?}");
    }
    Ok(())
}
