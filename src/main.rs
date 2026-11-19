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
    let api_key = env::var("API_KEY")
        .ok()
        .filter(|key| {
            let token = key.trim_end_matches('=');
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "API_KEY must be a nonempty bearer token",
            )
        })?;
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("Listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        spreadsheetpreview::router(env::current_exe()?, api_key),
    )
    .await
}
