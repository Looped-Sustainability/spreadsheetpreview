mod pdf;
mod render;
mod table;

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody, to_bytes};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    routing::{get, post},
};
use http_body_util::BodyExt;
use subtle::ConstantTimeEq;
use table::Options;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const UPLOAD: usize = 8 * 1024 * 1024;
const PDF_UPLOAD: u64 = 64 * 1024 * 1024;
const OLE: &[u8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";
type Failure = (StatusCode, &'static str);
const PDF_TOO_LARGE: Failure = (StatusCode::PAYLOAD_TOO_LARGE, "PDF exceeds 64 MiB");
const PDF_STORAGE_FAILED: Failure = (
    StatusCode::INTERNAL_SERVER_ERROR,
    "could not store PDF upload",
);

#[derive(Clone)]
struct Service {
    slots: Arc<Semaphore>,
    executable: PathBuf,
}

impl Service {
    fn acquire_slot(&self) -> Result<OwnedSemaphorePermit, Failure> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "preview workers are busy"))
    }

    async fn upload(&self, body: Body) -> Result<(OwnedSemaphorePermit, Bytes), Failure> {
        let permit = self.acquire_slot()?;
        let bytes = tokio::time::timeout(Duration::from_secs(15), to_bytes(body, UPLOAD))
            .await
            .map_err(|_| (StatusCode::REQUEST_TIMEOUT, "upload timed out"))?
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "upload failed or exceeds 8 MiB",
                )
            })?;
        if bytes.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "empty upload"));
        }
        Ok((permit, bytes))
    }
}

pub fn router(executable: PathBuf, api_key: String) -> Router {
    Router::new()
        .route("/preview", post(preview))
        .route("/pdf_render", post(pdf_render))
        .route_layer(middleware::from_fn_with_state(
            Arc::<str>::from(api_key),
            require_api_key,
        ))
        .route("/", get(api_guide))
        .route("/healthz", get(health))
        .with_state(Service {
            slots: Arc::new(Semaphore::new(2)),
            executable,
        })
}

#[derive(serde::Serialize)]
struct ApiGuide {
    markdown: &'static str,
}

async fn api_guide() -> Json<ApiGuide> {
    let markdown = include_str!("../README.md")
        .split_once("<!-- api-guide:start -->")
        .and_then(|(_, rest)| rest.split_once("<!-- api-guide:end -->"))
        .expect("README must contain the API guide")
        .0
        .trim();
    Json(ApiGuide { markdown })
}

async fn require_api_key(
    State(api_key): State<Arc<str>>,
    request: Request,
    next: Next,
) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let credentials = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '));
    if let Some((scheme, token)) = credentials
        && values.next().is_none()
        && scheme.eq_ignore_ascii_case("Bearer")
        && bool::from(
            token
                .trim_start_matches(' ')
                .as_bytes()
                .ct_eq(api_key.as_bytes()),
        )
    {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        "a valid bearer API key is required",
    )
        .into_response()
}

#[derive(serde::Serialize)]
struct Health {
    status: &'static str,
    revision: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        revision: option_env!("BUILD_REVISION").unwrap_or("development"),
    })
}

async fn preview(
    State(service): State<Service>,
    Query(options): Query<Options>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, Failure> {
    options.bounds().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let (permit, bytes) = service.upload(body).await?;
    let mime = media_type(&headers);
    let excel = bytes.starts_with(OLE) || bytes.starts_with(b"PK\x03\x04");
    match mime.as_str() {
        "" | "application/octet-stream" | "binary/octet-stream" => (),
        "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            if !excel =>
        {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, "invalid Excel workbook"));
        }
        "text/csv" | "application/csv" | "text/plain" if excel => {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, "expected CSV text"));
        }
        "text/csv"
        | "application/csv"
        | "text/plain"
        | "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => (),
        _ => {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "send a raw CSV, XLS, or XLSX file",
            ));
        }
    }
    let image = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if excel {
            workbook_process(&service.executable, &bytes, &options)
        } else {
            let table =
                table::csv(&bytes, &options).map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
            render::png(&table).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
        }
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "preview failed"))??;
    Ok(image_response("image/png", image))
}

async fn pdf_render(
    State(service): State<Service>,
    Query(options): Query<pdf::Options>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, Failure> {
    if !matches!(
        media_type(&headers).as_str(),
        "" | "application/pdf" | "application/octet-stream"
    ) {
        return Err((StatusCode::UNSUPPORTED_MEDIA_TYPE, "send a raw PDF file"));
    }
    let permit = service.acquire_slot()?;
    let file = tokio::time::timeout(Duration::from_secs(60), spool_pdf(body))
        .await
        .map_err(|_| (StatusCode::REQUEST_TIMEOUT, "PDF upload timed out"))??;
    let image = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let output = run_worker(
            Command::new(service.executable)
                .arg("--pdf")
                .arg(options.format.name()),
            WorkerInput::File(file),
        )?;
        if !output.status.success() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid PDF or PDF exceeds reader limits",
            ));
        }
        Ok(output.stdout)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "PDF rendering failed"))??;
    Ok(image_response(options.format.mime(), image))
}

async fn spool_pdf(mut body: Body) -> Result<File, Failure> {
    if body.size_hint().lower() > PDF_UPLOAD {
        return Err(PDF_TOO_LARGE);
    }
    let file = tokio::task::spawn_blocking(tempfile::tempfile)
        .await
        .map_err(|_| PDF_STORAGE_FAILED)?
        .map_err(|_| PDF_STORAGE_FAILED)?;
    let mut file = BufWriter::with_capacity(128 * 1024, tokio::fs::File::from_std(file));
    let mut size = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| (StatusCode::BAD_REQUEST, "could not read PDF upload"))?;
        if let Some(bytes) = frame.data_ref() {
            size += bytes.len() as u64;
            if size > PDF_UPLOAD {
                return Err(PDF_TOO_LARGE);
            }
            file.write_all(bytes)
                .await
                .map_err(|_| PDF_STORAGE_FAILED)?;
        }
    }
    if size == 0 {
        return Err((StatusCode::BAD_REQUEST, "empty upload"));
    }
    file.flush().await.map_err(|_| PDF_STORAGE_FAILED)?;
    Ok(file.into_inner().into_std().await)
}

fn media_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn image_response(mime: &'static str, image: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store"),
        ],
        image,
    )
        .into_response()
}

fn workbook_process(executable: &Path, data: &[u8], options: &Options) -> Result<Vec<u8>, Failure> {
    let (rows, cols) = options.bounds().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let output = run_worker(
        Command::new(executable)
            .arg("--workbook")
            .arg(rows.to_string())
            .arg(cols.to_string())
            .args(&options.sheet),
        WorkerInput::Bytes(data),
    )?;
    if !output.status.success() {
        let message = match output.stdout.as_slice() {
            b"worksheet not found" => "worksheet not found",
            b"expanded XLSX exceeds 64 MiB" => "expanded XLSX exceeds 64 MiB",
            b"too many XLSX parts" => "too many XLSX parts",
            _ => "invalid workbook or workbook exceeds reader limits",
        };
        return Err((StatusCode::UNPROCESSABLE_ENTITY, message));
    }
    Ok(output.stdout)
}

enum WorkerInput<'a> {
    Bytes(&'a [u8]),
    File(File),
}

fn run_worker(command: &mut Command, input: WorkerInput<'_>) -> Result<Output, Failure> {
    let bytes = match input {
        WorkerInput::Bytes(bytes) => {
            command.stdin(Stdio::piped());
            Some(bytes)
        }
        WorkerInput::File(file) => {
            command.stdin(file);
            None
        }
    };
    let mut child = command
        .env_remove("API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start render worker",
            )
        })?;
    let write = bytes
        .map(|bytes| child.stdin.take().unwrap().write_all(bytes))
        .transpose();
    let output = child
        .wait_with_output()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "render worker failed"))?;
    if output.status.code() == Some(3) || (output.status.success() && write.is_err()) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "render worker setup failed",
        ));
    }
    Ok(output)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn limit_worker() -> bool {
    let set = |resource, value| {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        unsafe { libc::setrlimit(resource, &limit) == 0 }
    };
    if !set(libc::RLIMIT_CORE, 0) || !set(libc::RLIMIT_CPU, 5) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        set(libc::RLIMIT_AS, 512 * 1024 * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        // Darwin does not enforce RLIMIT_RSS. Check resident memory in the isolated reader.
        std::thread::spawn(|| {
            loop {
                let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
                let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
                unsafe {
                    let read = libc::proc_pidinfo(
                        libc::getpid(),
                        libc::PROC_PIDTASKINFO,
                        0,
                        info.as_mut_ptr().cast(),
                        size,
                    );
                    if read != size || info.assume_init().pti_resident_size > 512 * 1024 * 1024 {
                        libc::_exit(2);
                    }
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        });
        true
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn limit_worker() -> bool {
    compile_error!(
        "spreadsheetpreview supports Linux and macOS. Render workers require platform-specific resource limits."
    )
}

pub fn worker(args: &[String]) -> i32 {
    if !limit_worker() {
        return 3;
    }
    let Some((mode, args)) = args.split_first() else {
        return 2;
    };
    let image = match mode.as_str() {
        "--workbook" => render_workbook(args),
        "--pdf" => match args.first().map(String::as_str) {
            Some("png") => pdf::render_upload(pdf::Format::Png, PDF_UPLOAD),
            Some("jpeg") => pdf::render_upload(pdf::Format::Jpeg, PDF_UPLOAD),
            _ => return 2,
        },
        _ => return 2,
    };
    match image {
        Ok(bytes) if std::io::stdout().write_all(&bytes).is_ok() => 0,
        Err(message) => {
            let _ = std::io::stdout().write_all(message.as_bytes());
            2
        }
        _ => 2,
    }
}

fn render_workbook(args: &[String]) -> Result<Vec<u8>, &'static str> {
    let (Some(rows), Some(cols)) = (
        args.first().and_then(|s| s.parse().ok()),
        args.get(1).and_then(|s| s.parse().ok()),
    ) else {
        return Err("invalid worksheet bounds");
    };
    let options = Options {
        sheet: args.get(2).cloned(),
        max_rows: Some(rows),
        max_cols: Some(cols),
    };
    let mut data = Vec::new();
    std::io::stdin()
        .take(UPLOAD as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| "could not read workbook")?;
    if data.len() > UPLOAD {
        return Err("workbook exceeds 8 MiB");
    }
    table::excel(&data, &options).and_then(|table| render::png(&table))
}

pub fn is_worker(arg: Option<&OsStr>) -> bool {
    arg == Some(OsStr::new("--workbook")) || arg == Some(OsStr::new("--pdf"))
}
