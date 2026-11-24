//! A TCP relay a test can cut, to drop a link the way a network does.

use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Relays every connection on its own port to `upstream`, until [`Relay::cut`] drops them
/// all. It keeps accepting new connections after a cut.
pub struct Relay {
    addr: SocketAddr,
    cut: watch::Sender<u64>,
    task: JoinHandle<()>,
}

impl Relay {
    pub async fn start(upstream: SocketAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (cut, _) = watch::channel(0u64);
        let cuts = cut.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut down, _)) = listener.accept().await {
                let mut rx = cuts.subscribe();
                tokio::spawn(async move {
                    let Ok(mut up) = TcpStream::connect(upstream).await else {
                        return;
                    };
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut down, &mut up) => {}
                        _ = rx.changed() => {}
                    }
                });
            }
        });
        Ok(Self { addr, cut, task })
    }

    /// Where to connect.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Drops every connection open now.
    pub fn cut(&self) {
        self.cut.send_modify(|n| *n += 1);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}
