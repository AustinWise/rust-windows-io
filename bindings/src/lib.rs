::windows::include_bindings!();

use std::convert::TryInto;

use windows::IntoParam;
use windows::Param;

use crate::Windows::Win32::SystemServices::{HANDLE, OVERLAPPED, OVERLAPPED_0};

impl<'a> IntoParam<'a, HANDLE> for std::os::windows::io::RawSocket {
    fn into_param(self) -> Param<'a, HANDLE> {
        Param::Owned(HANDLE(self.try_into().unwrap()))
    }
}

impl Default for OVERLAPPED {
    fn default() -> OVERLAPPED {
        OVERLAPPED {
            Anonymous: OVERLAPPED_0 {
                Anonymous: Default::default(),
            },
            hEvent: Default::default(),
            Internal: 0,
            InternalHigh: 0,
        }
    }
}
