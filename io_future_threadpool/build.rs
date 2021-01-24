fn main() {
    windows::build!(
        windows::win32::file_system::SetFileCompletionNotificationModes
        windows::win32::system_services::{
            CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, StartThreadpoolIo,
            ERROR_IO_PENDING, HANDLE, OVERLAPPED, TP_CALLBACK_INSTANCE, TP_IO,
        }
        windows::win32::win_sock::{LPFN_ACCEPTEX, WSAIoctl, WSARecv, WSASend, WSABUF}
        windows::win32::debug::GetLastError
    );
}
