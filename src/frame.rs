#[derive(Debug)]
pub struct BlockInfo {
    pub id: usize, // should be identical for each block. We can use the stream_id where the block is sent in Stream mode.
    pub size: usize,
    pub priority: usize,
    pub deadline: usize,
}

#[derive(Debug)]
pub struct Block {
    info: BlockInfo,
    offset: usize,
    data: Vec<u8>
}

pub enum StreamFrameType {
    NONE = 0x0,
    DTP_CONFIG = 0x1,
    BLOCK_INFO = 0x2,
    BLOCK_DATA = 0x3,
}

#[derive(Clone, PartialEq, Eq)]
pub enum StreamFrame {
    DtpConfig {
        cfg_len: usize, // the number of config blocks
    },
    BlockInfo {
        id: usize,
        size: usize,
        priority: usize,
        deadline: usize,
    },
    BlockData {
        id: usize,
        data: Vec<u8> // data is always continous, so we can keep it in a simple vector
    }
}
impl StreamFrame {
    pub fn from_bytes(
        frame_type: u64, payload_length: u64, bytes: &[u8],
    ) -> Result<Frame> {
        let frame = match frame_type {
            StreamFrameType::DTP_CONFIG => StreamFrame::DtpConfig {
                cfg_len: b.get_varint()?
            },
            StreamFrameType::BLOCK_INFO => StreamFrame::BlockInfo {
                id: b.get_varint()?,
                size: b.get_varint()?,
                priority: b.get_varint()?,
                deadline: b.get_varint()?,
            },
            _ => return Err(Error::InvalidFrame)
        };
        return Ok(frame);
    }
    pub fn to_bytes(&self, b: &mut octets::OctetsMut) -> Result<usize> {
        let before = b.cap();
        match self {
            StreamFrame::DtpConfig { cfg_len } => {
                b.put_varint(DATA_FRAME_TYPE_ID)?;
                b.put_varint(payload.len() as u64)?;

                b.put_bytes(payload.as_ref())?;
            },
        }
        Ok(before - b.cap())
    }
}