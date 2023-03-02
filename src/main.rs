use calloop::{timer::{Timer, TimeoutAction}, EventLoop, LoopSignal};
use std::{collections::{VecDeque, HashMap}, hash::Hash, net::TcpStream, io::Write};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct BlockInfo {
    pub id:       usize,
    pub size:     usize,
    pub priority: usize,
    pub deadline: usize 
}
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
struct SenderDeque {
    queue: VecDeque<SenderBlock>
}
impl SenderDeque {
    fn next_block_to_send(&self) -> Option<&SenderBlock>{
        return self.queue.front();
    }
    fn next_block_to_send_mut(&mut self) -> Option<&mut SenderBlock>{
        return self.queue.front_mut();
    }
    fn remove_block(&mut self, _block: &SenderBlock) -> Option<SenderBlock> {
        return self.queue.pop_front();
    }
    fn len(&self) -> usize {
        return self.queue.len();
    }
}
fn send_data(sender_queue: &mut SenderDeque, tcp_map: &mut HashMap<usize, TcpStream>) -> Result<usize, String>{
    loop {
        // select a block to send
        let block = sender_queue.next_block_to_send_mut().unwrap();
        // mark whether the block is sent first time
        if !block.has_begun_sending() {
            block.begin_sending();
        }
        // try to send data into socket, record the total bytes sent
        send_block_to_tcp(block, tcp_map);
        // if the data has been sent completely, pop the block from the queue
        if block.is_send_complete() {
            sender_queue.remove_block(block);
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
fn generate_data() {

}
fn main() {
    // Create the event loop. The loop is parameterised by the kind of shared
    // data you want the callbacks to use. In this case, we want to be able to
    // stop the loop when the timer fires, so we provide the loop with a
    // LoopSignal, which has the ability to stop the loop from within events. We
    // just annotate the type here; the actual data is provided later in the
    // run() call.
    let mut event_loop: EventLoop<LoopSignal> =
        EventLoop::try_new().expect("Failed to initialize the event loop!");

    // Retrieve a handle. It is used to insert new sources into the event loop
    // It can be cloned, allowing you to insert sources from within source
    // callbacks.
    let handle = event_loop.handle();

    // Create our event source, a timer, that will expire in 2 seconds
    let source = Timer::from_duration(std::time::Duration::from_secs(2));

    // Inserting an event source takes this general form. It can also be done
    // from within the callback of another event source.
    handle
        .insert_source(
            // a type which implements the EventSource trait
            source,
            // a callback that is invoked whenever this source generates an event
            |event, _metadata, shared_data| {
                // This callback is given 3 values:
                // - the event generated by the source (in our case, timer events are the Instant
                //   representing the deadline for which it has fired)
                // - &mut access to some metadata, specific to the event source (in our case, a
                //   timer handle)
                // - &mut access to the global shared data that was passed to EventLoop::run or
                //   EventLoop::dispatch (in our case, a LoopSignal object to stop the loop)
                //
                // The return type is just () because nothing uses it. Some
                // sources will expect a Result of some kind instead.
                println!("Timeout for {:?} expired!", event);
                // notify the event loop to stop running using the signal in the shared data
                // (see below)
                shared_data.stop();
                // The timer event source requires us to return a TimeoutAction to
                // specify if the timer should be rescheduled. In our case we just drop it.
                TimeoutAction::Drop
            },
        )
        .expect("Failed to insert event source!");

    // Create the shared data for our loop.
    let mut shared_data = event_loop.get_signal();

    // Actually run the event loop. This will dispatch received events to their
    // callbacks, waiting at most 20ms for new events between each invocation of
    // the provided callback (pass None for the timeout argument if you want to
    // wait indefinitely between events).
    //
    // This is where we pass the *value* of the shared data, as a mutable
    // reference that will be forwarded to all your callbacks, allowing them to
    // share some state
    event_loop
        .run(
            std::time::Duration::from_millis(20),
            &mut shared_data,
            |_shared_data| {
                // Finally, this is where you can insert the processing you need
                // to do do between each waiting event eg. drawing logic if
                // you're doing a GUI app.
            },
        )
        .expect("Error during event loop!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something() {

    }
}
