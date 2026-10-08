//! `gethostname()` (what OSC 7 carries) and `uname -s -m`.

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable and its length is passed.
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 { return String::new(); }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

pub fn uname() -> String {
    // SAFETY: utsname is plain data filled by uname(2).
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 { return String::new(); }
    let f = |s: &[libc::c_char]| unsafe { std::ffi::CStr::from_ptr(s.as_ptr()) }.to_string_lossy().into_owned();
    format!("{} {}", f(&u.sysname), f(&u.machine))
}

pub fn tty() -> Option<String> {
    // SAFETY: ttyname returns static storage or null.
    let p = unsafe { libc::ttyname(0) };
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}
