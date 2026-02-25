use super::rclserver::RCLServer;

pub struct RCLLock<'a, T> {
    server: &'a RCLServer,
    data: T,
}
