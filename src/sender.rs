use crate::block::BlockInfo;
use std::collections::{VecDeque, HashMap};
use std::net::TcpStream;
use std::io::Write;
use dtp_utils::dtp_config;
use rand::Rng;

#[derive(Debug, Clone)]
#[repr(C)]
pub struct SenderBlock {
    pub info: BlockInfo,
    pub data: Vec<u8>,
    sent_size: usize,
    has_begun: bool,
}
impl SenderBlock {
    fn new(info: BlockInfo) -> Self {
        SenderBlock {
            info,
            data: Vec::new(),
            sent_size: 0,
            has_begun: false
        }
    }
    fn has_begun_sending(&self) -> bool {
        return self.has_begun;
    }
    fn begin_sending(&mut self) {
        self.has_begun = true;
    }
    fn is_send_complete(&self) -> bool {
        return self.info.size == self.sent_size;
    }
    fn send_bytes(&mut self, bytes: usize) {
        assert!(self.sent_size + bytes <= self.info.size);
        self.sent_size += bytes;
    }
    fn remain_bytes(&self) -> usize {
        return self.info.size - self.sent_size;
    }
}
#[derive(Default)]
pub struct SenderDeque {
    queue: VecDeque<SenderBlock>
}
impl SenderDeque {
    fn next_block_to_send(&self) -> Option<&SenderBlock>{
        return self.queue.front();
    }
    fn next_block_to_send_mut(&mut self) -> Option<&mut SenderBlock>{
        return self.queue.front_mut();
    }
    fn remove_block(&mut self) -> Option<SenderBlock> {
        return self.queue.pop_front();
    }
    fn len(&self) -> usize {
        return self.queue.len();
    }
}
fn send_data(sender_queue: &mut SenderDeque, tcp_map: &mut HashMap<usize, TcpStream>) -> Result<usize, std::io::Error>{
    loop {
        // select a block to send
        let block = sender_queue.next_block_to_send_mut().unwrap();
        // mark whether the block is sent first time
        if !block.has_begun_sending() {
            block.begin_sending();
        }
        // try to send data into socket, record the total bytes sent
        match send_block_to_tcp(block, tcp_map) {
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                return Ok(sender_queue.len());
            },
            Err(err) => return Err(err),
            Ok(_) => {},
        }
        // if the data has been sent completely, pop the block from the queue
        if block.is_send_complete() {
            sender_queue.remove_block();
        } else {
            // or leave the function and wait until the connection is ready
            return Ok(sender_queue.len());
        }
    }
}
fn send_block_to_tcp(block: &mut SenderBlock, tcp_map: &mut HashMap<usize, TcpStream>) -> Result<usize, std::io::Error>{
    let socket = tcp_map.get_mut(&block.info.id).unwrap();
    let buf = &block.data[block.sent_size..block.info.size];
    let sent = socket.write(buf)?;
    block.send_bytes(sent);
    return Ok(sent);
}

#[derive(Default)]
pub struct BlockGenerator {
    cfgs: Vec<dtp_config>,
    next_index_to_generate: usize,
}

impl BlockGenerator {
    pub fn load_cfgs(&mut self, cfgs: Vec<dtp_config>) {
        self.cfgs = cfgs;
    }
    /// Generate a block to sender queue
    /// Should be called again after secs of the return value
    /// return next time gap, None if no more block to generate
    pub fn generate_once(&mut self, sender_queue: &mut SenderDeque) -> Option<f32> {
        let mut rng = rand::thread_rng();
        static mut RANDOM_BUFFER: [u8; 10000000] = [0u8; 10000000];
        let start = self.next_index_to_generate;
        for (index, cfg ) in self.cfgs[start..].iter().enumerate() {
            debug!("generate: ({}, {}, {}, {}, {})", self.next_index_to_generate, cfg.send_time_gap, cfg.block_size, cfg.priority, cfg.deadline);
            let mut sender_block = 
                SenderBlock::new(
                    BlockInfo {
                        id: self.next_index_to_generate,
                        size: cfg.block_size as usize,
                        priority: cfg.priority as usize,
                        deadline: cfg.priority as usize,
                    }
                );
            unsafe {
                let buf=  &mut RANDOM_BUFFER[..sender_block.info.size];
                rng.fill(buf);
                sender_block.data = buf.to_vec();
            }
            sender_queue.queue.push_back(sender_block);
            self.next_index_to_generate += 1;
        
            if self.next_index_to_generate >= self.cfgs.len() {
                return None
            }

            let next_cfg = self.cfgs[self.next_index_to_generate];
        
            // if send gap is too small
            // we generate the data immediately to avoid
            // timer error
            if next_cfg.send_time_gap <= 0.000001 {
                continue;
            } else {
                return Some(next_cfg.send_time_gap);
            }
        }
        None
    }
    
    pub fn first_time_gap(&self) -> Option<f32> {
        if self.cfgs.len() <= 0 {
            None
        } else {
            Some(self.cfgs[0].send_time_gap)
        }
    }
}