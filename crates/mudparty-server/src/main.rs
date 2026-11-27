//! `mudparty-server [--root <data root>]`: starts the server on a data root (default: the
//! current folder), serves the web page on `listen.client_hub` and the MudPlay proxy on
//! `listen.proxy`, runs until Ctrl-C, then logs every bot off.
//!
//! `mudparty-server places compile [--area <name>] [--root <data root>]`: writes the
//! compiled room files into `places/` (design §14.4) and exits.
//!
//! `mudparty-server check <file> [--level <n>] [--root <data root>]`: prints a character,
//! routine or places local file as the server reads it, and what would stop it (design
//! §14.9); exits 1 when something would.
//!
//! `mudparty-server schema [--out <folder>]`: writes the JSON Schemas for the files written
//! by hand, into `schemas/` under the data root unless told where (design §14.9).

use std::path::PathBuf;

use anyhow::bail;
use mudparty_server::{Server, check, compile_places, proxy, web, write_schemas};

const USAGE: &str = "mudparty-server [--root <data root>]
mudparty-server places compile [--area <name>] [--root <data root>]
mudparty-server check <file> [--level <n>] [--root <data root>]
mudparty-server schema [--out <folder>] [--root <data root>]";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from(".");
    let mut words: Vec<String> = Vec::new();
    let mut area: Option<String> = None;
    let mut level: Option<u32> = None;
    let mut out: Option<PathBuf> = None;
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
            "--level" => match args.next().map(|n| n.parse()) {
                Some(Ok(n)) => level = Some(n),
                _ => bail!("--level needs the character's level, a number"),
            },
            "--out" => match args.next() {
                Some(o) => out = Some(PathBuf::from(o)),
                None => bail!("--out needs a folder"),
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
        [] if area.is_none() && level.is_none() && out.is_none() => {}
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
        ["check", file] => {
            let checked = check(&root, &PathBuf::from(file), level)?;
            print!("{}", checked.text);
            if level.is_none() {
                eprintln!("mudparty-server: level exits not checked; give --level to check them");
            }
            if checked.problems.is_empty() {
                eprintln!("mudparty-server: {file} is fine");
                return Ok(());
            }
            for p in &checked.problems {
                eprintln!("{p}");
            }
            std::process::exit(1);
        }
        ["schema"] => {
            let dir = out.unwrap_or_else(|| root.join("schemas"));
            for path in write_schemas(&dir)? {
                println!("mudparty-server: wrote {}", path.display());
            }
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
