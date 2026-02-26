pub mod client;
pub mod protocol;
pub mod server;

pub use client::IpcApplicationClient;
pub use protocol::{Request, Response};
pub use server::{ApplicationHandler, IpcApplicationServer};
