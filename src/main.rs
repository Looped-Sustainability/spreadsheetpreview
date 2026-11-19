use std::{env, io};

fn main() -> io::Result<()> {
    if spreadsheetpreview::is_worker(env::args_os().nth(1).as_deref()) {
        let args: Vec<_> = env::args().skip(2).collect();
        std::process::exit(spreadsheetpreview::worker(&args));
    }
    serve()
}

#[tokio::main]
async fn serve() -> io::Result<()> {
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("Listening on {}", listener.local_addr()?);
    axum::serve(listener, spreadsheetpreview::router(env::current_exe()?)).await
}
