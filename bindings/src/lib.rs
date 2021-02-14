::windows::include_bindings!();

use std::convert::TryInto;

use crate::windows::win32::system_services::HANDLE;

impl From<std::os::windows::io::RawSocket> for HANDLE {
    fn from(sock: std::os::windows::io::RawSocket) -> Self {
        HANDLE(sock.try_into().unwrap())
    }
}
