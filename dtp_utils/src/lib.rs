use std::{ffi::c_void, slice, ffi::CString};

use libc::{free};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct dtp_config {
  pub deadline: i32, // in milliseconds
  pub priority: i32,
  pub block_size: i32,  // in bytes
  pub send_time_gap: f32 // in seconds
}

extern "C" {
  fn getCurrentUsec() -> u64;
  fn parse_dtp_config(filename: *const libc::c_char, number: *mut i32) -> * const dtp_config;
}

#[allow(dead_code)]
/// Get a vector of dtp_configs
/// 
/// The function is memory safe
pub fn get_dtp_config(filename: &str) -> Vec<dtp_config> {
  unsafe {
    // get dtp_config raw pointer
    let mut number = 0;
    let filename_to_c = CString::new(filename).unwrap();
    let cfgs_ptr: * const dtp_config = parse_dtp_config(filename_to_c.as_ptr(), &mut number);
    if !cfgs_ptr.is_null(){
      // copy and save them in Rust vector
      let cfgs_slice = slice::from_raw_parts(cfgs_ptr, number as usize);
      let cfgs_vec = cfgs_slice.to_vec();
      // free the raw pointer
      free(cfgs_ptr as *mut c_void);
      return cfgs_vec;
    } else {
      return Vec::new();
    }
  }
}

#[allow(dead_code)]
/// A Rust wrapper of C 'getCurrentUsec' function
pub fn get_current_usec() -> u64 {
  unsafe {
    getCurrentUsec()
  }
}
