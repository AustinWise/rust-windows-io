fn main() {
    windows::build!(
        windows::win32::win_sock::{WSAGetLastError, WSASend, WSARecv}
        windows::win32::file_system::{SetFileCompletionNotificationModes, CreateIoCompletionPort, GetQueuedCompletionStatus}
        windows::win32::system_services::ERROR_IO_PENDING
        windows::win32::windows_programming::CloseHandle
    );
}
