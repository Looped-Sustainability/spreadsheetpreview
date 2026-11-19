mod render;
mod table;

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    routing::{get, post},
};
use subtle::ConstantTimeEq;
use table::Options;
use tokio::sync::Semaphore;

const UPLOAD: usize = 8 * 1024 * 1024;
const OLE: &[u8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";
type Failure = (StatusCode, &'static str);

#[derive(Clone)]
struct Service {
    slots: Arc<Semaphore>,
    executable: PathBuf,
}

pub fn router(executable: PathBuf, api_key: String) -> Router {
    Router::new()
        .route("/preview", post(preview))
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

async fn api_guide() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("api-guide.json"),
    )
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
    let permit = service
        .slots
        .try_acquire_owned()
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "preview workers are busy"))?;
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
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
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
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        image,
    )
        .into_response())
}

fn workbook_process(executable: &Path, data: &[u8], options: &Options) -> Result<Vec<u8>, Failure> {
    let (rows, cols) = options.bounds().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let mut child = Command::new(executable)
        .env_remove("API_KEY")
        .arg("--workbook")
        .arg(rows.to_string())
        .arg(cols.to_string())
        .args(&options.sheet)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start workbook reader",
            )
        })?;
    let write = child.stdin.take().unwrap().write_all(data);
    let output = child
        .wait_with_output()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "workbook reader failed"))?;
    if output.status.code() == Some(3) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not limit workbook reader",
        ));
    }
    if !output.status.success() || write.is_err() {
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
        "spreadsheetpreview supports Linux and macOS. The workbook reader requires platform-specific resource limits."
    )
}

pub fn worker(args: &[String]) -> i32 {
    if !limit_worker() {
        return 3;
    }
    let (Some(rows), Some(cols)) = (
        args.first().and_then(|s| s.parse().ok()),
        args.get(1).and_then(|s| s.parse().ok()),
    ) else {
        return 2;
    };
    let options = Options {
        sheet: args.get(2).cloned(),
        max_rows: Some(rows),
        max_cols: Some(cols),
    };
    let mut data = Vec::new();
    if std::io::stdin()
        .take(UPLOAD as u64 + 1)
        .read_to_end(&mut data)
        .is_err()
        || data.len() > UPLOAD
    {
        return 2;
    }
    match table::excel(&data, &options).and_then(|table| render::png(&table)) {
        Ok(bytes) if std::io::stdout().write_all(&bytes).is_ok() => 0,
        Err(message) => {
            let _ = std::io::stdout().write_all(message.as_bytes());
            2
        }
        _ => 2,
    }
}

pub fn is_worker(arg: Option<&OsStr>) -> bool {
    arg == Some(OsStr::new("--workbook"))
}
