// Hide console window in Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod vmcompute;

#[cfg(windows)]
#[path = "vmsocket.windows.rs"]
mod vmsocket;

#[cfg(unix)]
#[path = "vmsocket.linux.rs"]
mod vmsocket;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::pin;

#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(windows)]
use uuid::Uuid;

#[cfg(unix)]
use tokio::net::UnixListener;

use vmsocket::VmSocket;

async fn connect_stream<R: AsyncRead, W: AsyncWrite>(r: R, w: W) -> std::io::Result<()> {
    pin!(r);
    pin!(w);
    let mut buf = vec![0u8; 4096];
    loop {
        let size = r.read(&mut buf).await?;
        if size == 0 {
            break;
        }
        w.write_all(&buf[0..size]).await?;
    }
    w.shutdown().await
}

#[cfg(windows)]
async fn task(vmid: Uuid) -> std::io::Result<()> {
    let listener = VmSocket::bind(vmid, 6000).await?;

    loop {
        let (client_r, client_w) = listener.accept().await?.into_split();

        tokio::task::spawn(async move {
            let result = async {
                let server = TcpStream::connect("localhost:6000").await?;
                server.set_nodelay(true)?;
                let (server_r, server_w) = server.into_split();
                let a = tokio::task::spawn(connect_stream(client_r, server_w));
                let b = tokio::task::spawn(connect_stream(server_r, client_w));
                a.await.unwrap()?;
                b.await.unwrap()
            }
            .await;
            if let Err(err) = result {
                eprintln!("Failed to transfer: {}", err);
            }
        });
    }
}

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    unsafe { winapi::um::wincon::AttachConsole(winapi::um::wincon::ATTACH_PARENT_PROCESS) };

    let vmid_arg = std::env::args().nth(1);

    if let Some("--daemon") = vmid_arg.as_deref() {
        let mut prev_vmid = None;
        let mut future: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            let vmid = tokio::task::spawn_blocking(|| vmcompute::get_wsl_vmid().unwrap())
                .await
                .unwrap();
            if vmid != prev_vmid {
                if let Some(future) = future.take() {
                    future.abort();
                }
                prev_vmid = vmid;
                if let Some(vmid) = vmid {
                    future = Some(tokio::task::spawn(async move {
                        // Three chances, to avoid a race between get_wsl_vmid and spawn.
                        for _ in 0..3 {
                            if let Err(err) = task(vmid).await {
                                eprintln!("Failed to listen: {}", err);
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        std::process::exit(1);
                    }));
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    } else {
        let vmid = match vmid_arg {
            Some(str) => str.parse().expect("VMID is not valid UUID"),
            None => vmcompute::get_wsl_vmid()
                .unwrap()
                .expect("WSL is not running"),
        };

        if let Err(err) = task(vmid).await {
            eprintln!("Failed to listen: {}", err);
            return;
        }
    }
}

#[cfg(unix)]
async fn task() -> std::io::Result<()> {
    let listener = UnixListener::bind("/tmp/.X11-unix/X0")?;

    loop {
        let (client_r, client_w) = listener.accept().await?.0.into_split();

        tokio::task::spawn(async move {
            let result = async {
                let (server_r, server_w) = VmSocket::connect(6000).await?.into_split();
                let a = tokio::task::spawn(connect_stream(client_r, server_w));
                let b = tokio::task::spawn(connect_stream(server_r, client_w));
                a.await.unwrap()?;
                b.await.unwrap()
            }
            .await;
            if let Err(err) = result {
                eprintln!("Failed to transfer: {}", err);
            }
        });
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Remove existing socket
    let _ = std::fs::create_dir_all("/tmp/.X11-unix");
    let _ = std::fs::remove_file("/tmp/.X11-unix/X0");

    if let Err(err) = task().await {
        eprintln!("Failed to listen: {}", err);
        return;
    }
}
