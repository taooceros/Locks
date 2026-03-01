use std::thread::{self, ThreadId};

use super::rclserver::RCLServer;

pub struct RCLThread<'a> {
    server: &'a RCLServer,
    timestamp: i32,
    pub wait_to_serve: ThreadId,
    pub thread_handle: Option<thread::JoinHandle<()>>,
}
