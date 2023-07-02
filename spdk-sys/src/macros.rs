/// Convert an SPDK integer status return value to a [`Result`].
/// 
/// A status return value either indicates success (0) or an error (<0). A
/// success return is converted to `Ok(())` while an error return is converted
/// to `Err(Errno)` containing the error code.
#[macro_export]
macro_rules! to_result {
    ($r:expr) => {
        match Errno($r) {
            Errno(0) => Ok(()),
            Errno(e) if e < 0 => Err(Errno(-e)),
            _ => unreachable!()
        }
    };
}

/// Convert an SPDK integer size return value to a [`Result`].
/// 
/// Some SPDK functions return a size on success. This is indicated by a return
/// value that is `>= 0`. A success return is converted to `Ok(size)` where
/// `size` is the positive integer value returned. An error return (`< 0`) is
/// converted to `Err(Errno)` containing the error code.
#[macro_export]
macro_rules! to_result_size {
    ($r:expr) => {
        match Errno($r) {
            Errno(e) if e < 0 => Err(Errno(-e)),
            Errno(s) => Ok(s as usize)
        }
    };
}
