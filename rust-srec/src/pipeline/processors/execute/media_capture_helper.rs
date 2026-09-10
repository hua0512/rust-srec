//! Standalone, std-only compatibility helper. Records argv; never evaluates it.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn write_arguments(file: &mut impl Write, tag: &[u8; 4], values: &[String]) -> io::Result<()> {
    if values.len() > 64 || values.iter().any(|value| value.len() > 1024 * 1024) {
        return Err(invalid("capture size exceeds the fixture contract"));
    }
    file.write_all(tag)?;
    file.write_all(&(values.len() as u32).to_le_bytes())?;
    for value in values {
        file.write_all(&(value.len() as u32).to_le_bytes())?;
        file.write_all(value.as_bytes())?;
    }
    Ok(())
}

#[cfg(windows)]
mod shell32 {
    use std::ffi::c_void;
    use std::io;
    use std::ptr::NonNull;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCommandLineW() -> *const u16;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
    }

    struct Allocation(Option<NonNull<*mut u16>>);

    impl Allocation {
        fn free(&mut self) -> io::Result<()> {
            let Some(pointer) = self.0 else {
                return Ok(());
            };
            // SAFETY: this is the one allocation returned by CommandLineToArgvW;
            // all strings have been copied before a successful explicit free.
            if unsafe { LocalFree(pointer.as_ptr().cast()) }.is_null() {
                self.0 = None;
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }

    impl Drop for Allocation {
        fn drop(&mut self) {
            if let Err(error) = self.free() {
                eprintln!("CommandLineToArgvW allocation cleanup failed: {error}");
            }
        }
    }

    pub(super) fn arguments() -> io::Result<Vec<String>> {
        let mut count = 0;
        // SAFETY: GetCommandLineW returns the process's live terminated string;
        // count is writable, and the API returns an independently owned block.
        let pointer = NonNull::new(unsafe { CommandLineToArgvW(GetCommandLineW(), &mut count) })
            .ok_or_else(io::Error::last_os_error)?;
        let mut allocation = Allocation(Some(pointer));
        if !(3..=67).contains(&count) {
            return Err(super::invalid(
                "Shell32 argument count exceeds the fixture contract",
            ));
        }
        // SAFETY: the successful API call supplies exactly count string pointers,
        // all retained by allocation until copying and explicit free complete.
        let pointers = unsafe { std::slice::from_raw_parts(pointer.as_ptr(), count as usize) };
        let mut values = Vec::with_capacity(pointers.len() - 1);
        for &word in &pointers[1..] {
            if word.is_null() {
                return Err(super::invalid("Shell32 returned a null argument pointer"));
            }
            let mut length = 0;
            loop {
                if length == 32768 {
                    return Err(super::invalid(
                        "Shell32 argument exceeds the Windows command-line bound",
                    ));
                }
                // SAFETY: each API string is NUL-terminated inside the retained
                // block; Windows limits the originating command line to 32767 units.
                if unsafe { *word.add(length) } == 0 {
                    break;
                }
                length += 1;
            }
            // SAFETY: the preceding scan established this live argument's length.
            let wide = unsafe { std::slice::from_raw_parts(word, length) };
            values.push(
                String::from_utf16(wide)
                    .map_err(|_| super::invalid("non-Unicode Shell32 argument"))?,
            );
        }
        allocation.free()?;
        Ok(values)
    }
}

fn main() -> io::Result<()> {
    let mut arguments = env::args_os().skip(1);
    let root = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| invalid("missing fixture root"))?,
    );
    let output = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| invalid("missing capture path"))?,
    );
    if !root.is_absolute() || !output.is_absolute() {
        return Err(invalid("fixture paths must be absolute"));
    }
    let parent = output
        .parent()
        .ok_or_else(|| invalid("capture path has no parent"))?;
    if fs::canonicalize(parent)? != fs::canonicalize(&root)? {
        return Err(invalid(
            "capture must be a direct child of the fixture root",
        ));
    }
    let values: Vec<String> = arguments
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| invalid("non-Unicode argument"))
        })
        .collect::<io::Result<_>>()?;
    if values.len() > 64 || values.iter().any(|value| value.len() > 1024 * 1024) {
        return Err(invalid("capture size exceeds the fixture contract"));
    }
    #[cfg(windows)]
    let shell32_values = {
        let arguments = shell32::arguments()?;
        if PathBuf::from(&arguments[0]) != root || PathBuf::from(&arguments[1]) != output {
            return Err(invalid("Shell32 changed the fixed capture-path arguments"));
        }
        arguments.into_iter().skip(2).collect::<Vec<_>>()
    };
    // Length-prefixing records both word boundaries and exact UTF-8 bytes,
    // without depending on another command-line quoting or JSON implementation.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    write_arguments(&mut file, b"ARGV", &values)?;
    #[cfg(windows)]
    write_arguments(&mut file, b"SH32", &shell32_values)?;
    file.flush()
}
