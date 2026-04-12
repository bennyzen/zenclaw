#[cfg(feature = "desktop")]
mod http_client;
#[cfg(feature = "desktop")]
mod server;

#[cfg(feature = "desktop")]
pub use http_client::ReqwestHttpClient;
#[cfg(feature = "desktop")]
pub use server::start_api_server;
