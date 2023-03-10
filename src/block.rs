#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct BlockInfo {
    pub id:       usize,
    pub size:     usize,
    pub priority: usize,
    pub deadline: usize 
}