use std::{any::type_name, sync::Arc, time::Duration};

use futures::{AsyncReadExt, FutureExt};
use gpui::{
    App,
    http_client::{
        self, AsyncBody, HttpClient, RedirectPolicy, Request, Response, Url,
        http::{self, HeaderValue},
    },
};
use reqwest::redirect;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct ExplorerHttpClient {
    client: reqwest::Client,
    user_agent: HeaderValue,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl ExplorerHttpClient {
    fn new() -> http_client::Result<Self> {
        Self::new_with_system_proxy(true)
    }

    #[cfg(test)]
    fn new_without_system_proxy_for_test() -> http_client::Result<Self> {
        Self::new_with_system_proxy(false)
    }

    fn new_with_system_proxy(use_system_proxy: bool) -> http_client::Result<Self> {
        let user_agent = HeaderValue::from_str(&format!("Explorer/{}", env!("CARGO_PKG_VERSION")))?;
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::USER_AGENT, user_agent.clone());
        let mut client_builder = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .default_headers(headers);
        if !use_system_proxy {
            client_builder = client_builder.no_proxy();
        }
        let client = client_builder.build()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;

        Ok(Self {
            client,
            user_agent,
            runtime: Arc::new(runtime),
        })
    }
}

impl HttpClient for ExplorerHttpClient {
    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, http_client::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let runtime = self.runtime.handle().clone();

        async move {
            let (parts, mut body) = request.into_parts();
            let mut body_bytes = Vec::new();
            body.read_to_end(&mut body_bytes).await?;

            let mut request = client.request(parts.method, parts.uri.to_string());
            request = request.headers(parts.headers);
            if let Some(policy) = parts.extensions.get::<RedirectPolicy>() {
                request = request.redirect_policy(match policy {
                    RedirectPolicy::NoFollow => redirect::Policy::none(),
                    RedirectPolicy::FollowLimit(limit) => {
                        redirect::Policy::limited(*limit as usize)
                    }
                    RedirectPolicy::FollowAll => redirect::Policy::limited(100),
                });
            }
            if !body_bytes.is_empty() {
                request = request.body(body_bytes);
            }

            runtime
                .spawn(async move {
                    let mut response = request.send().await?;
                    let status = response.status();
                    let version = response.version();
                    let headers = std::mem::take(response.headers_mut());
                    let bytes = response.bytes().await?;
                    let mut builder = Response::builder().status(status).version(version);
                    *builder.headers_mut().expect("response builder headers") = headers;
                    Ok(builder.body(AsyncBody::from(bytes.to_vec()))?)
                })
                .await?
        }
        .boxed()
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }
}

pub(crate) fn initialize(cx: &mut App) {
    match ExplorerHttpClient::new() {
        Ok(client) => cx.set_http_client(Arc::new(client)),
        Err(error) => eprintln!("failed to initialize Explorer HTTP client: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn test_server(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let address = listener.local_addr().expect("test HTTP server address");
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept test HTTP request");
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                stream
                    .write_all(response.as_bytes())
                    .expect("write test HTTP response");
            }
        });
        format!("http://{address}")
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn client_has_explorer_user_agent() {
        let client = ExplorerHttpClient::new().expect("Explorer HTTP client");
        assert_eq!(
            client.user_agent().and_then(|value| value.to_str().ok()),
            Some(concat!("Explorer/", env!("CARGO_PKG_VERSION")))
        );
        assert!(client.type_name().contains("ExplorerHttpClient"));
    }

    #[gpui::test]
    fn initialize_installs_explorer_http_client(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            initialize(app);
            let client = app.http_client();
            assert!(client.type_name().contains("ExplorerHttpClient"));
            assert_eq!(
                client.user_agent().and_then(|value| value.to_str().ok()),
                Some(concat!("Explorer/", env!("CARGO_PKG_VERSION")))
            );
        });
    }

    #[test]
    fn client_get_returns_status_headers_and_body() {
        let server = test_server(vec![response(
            "200 OK",
            "Content-Type: image/png\r\n",
            "icon",
        )]);
        let client =
            ExplorerHttpClient::new_without_system_proxy_for_test().expect("Explorer HTTP client");
        let mut response =
            futures::executor::block_on(client.get(&format!("{server}/icon.png"), ().into(), true))
                .expect("successful GET");
        let mut body = Vec::new();
        futures::executor::block_on(response.body_mut().read_to_end(&mut body))
            .expect("read response body");

        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.headers()[http::header::CONTENT_TYPE], "image/png");
        assert_eq!(body, b"icon");
    }

    #[test]
    fn client_get_follows_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect test server");
        let address = listener.local_addr().expect("redirect test server address");
        thread::spawn(move || {
            for response in [
                response(
                    "302 Found",
                    &format!("Location: http://{address}/icon\r\n"),
                    "",
                ),
                response("200 OK", "Content-Type: image/png\r\n", "icon"),
            ] {
                let (mut stream, _) = listener.accept().expect("accept redirect test request");
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                stream
                    .write_all(response.as_bytes())
                    .expect("write redirect test response");
            }
        });

        let client =
            ExplorerHttpClient::new_without_system_proxy_for_test().expect("Explorer HTTP client");
        let response = futures::executor::block_on(client.get(
            &format!("http://{address}/redirect"),
            ().into(),
            true,
        ))
        .expect("redirected GET");
        assert_eq!(response.status(), http::StatusCode::OK);
    }

    #[test]
    fn client_get_connection_failure_returns_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused port");
        let address = listener.local_addr().expect("unused port address");
        drop(listener);

        let client =
            ExplorerHttpClient::new_without_system_proxy_for_test().expect("Explorer HTTP client");
        let result = futures::executor::block_on(client.get(
            &format!("http://{address}/missing"),
            ().into(),
            true,
        ));
        assert!(result.is_err());
    }
}
