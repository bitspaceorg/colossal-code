//! Build-time bubblewrap entrypoint.
//!
//! On Linux targets, the build script compiles bubblewrap's C sources and
//! exposes a `bwrap_main` symbol that we can call via FFI.  This eliminates
//! the external dependency on system-installed bubblewrap.

#[cfg(vendored_bwrap_available)]
mod imp {
    use std::ffi::CString;
    use std::os::raw::c_char;

    unsafe extern "C" {
        fn bwrap_main(argc: libc::c_int, argv: *const *const c_char) -> libc::c_int;
    }

    fn argv_to_cstrings(argv: &[String]) -> Vec<CString> {
        argv.iter()
            .map(|arg| {
                CString::new(arg.as_str())
                    .unwrap_or_else(|err| panic!("failed to convert argv to CString: {err}"))
            })
            .collect()
    }

    /// Run the build-time bubblewrap `main` function and return its exit code.
    ///
    /// On success, bubblewrap will `execve` into the target program and this
    /// function will never return.  A return value therefore implies failure.
    pub(crate) fn run_vendored_bwrap_main(argv: &[String]) -> libc::c_int {
        let cstrings = argv_to_cstrings(argv);
        let mut argv_ptrs: Vec<*const c_char> = cstrings.iter().map(|arg| arg.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        // SAFETY: We provide a null-terminated argv vector whose pointers
        // remain valid for the duration of the call.
        unsafe { bwrap_main(cstrings.len() as libc::c_int, argv_ptrs.as_ptr()) }
    }

    /// Execute the build-time bubblewrap `main` function with the given argv.
    pub fn exec_vendored_bwrap(argv: Vec<String>) -> ! {
        let exit_code = run_vendored_bwrap_main(&argv);
        std::process::exit(exit_code);
    }

    pub const VENDORED_BWRAP_AVAILABLE: bool = true;
}

#[cfg(not(vendored_bwrap_available))]
mod imp {
    /// Vendored bubblewrap is not available in this build.
    pub fn exec_vendored_bwrap(_argv: Vec<String>) -> ! {
        panic!(
            "vendored bubblewrap is not available in this build.\n\
             Install bubblewrap (bwrap) via your package manager, or ensure \
             libcap-dev is installed so the build script can compile vendored bubblewrap."
        );
    }

    pub const VENDORED_BWRAP_AVAILABLE: bool = false;
}

pub use imp::VENDORED_BWRAP_AVAILABLE;
pub use imp::exec_vendored_bwrap;
