#[macro_use]
extern crate log;
use std::{error::Error, net::UdpSocket};
use calloop::{timer::{Timer, TimeoutAction}, EventLoop, LoopSignal, LoopHandle, RegistrationToken};
use calloop::{generic::Generic, Interest, Mode, PostAction, Dispatcher};
use std::{collections::{VecDeque, HashMap}, hash::Hash, net::TcpStream, net::TcpListener, io::Write};
use anyhow::{anyhow, Result};
use rand::Rng;
use std::time::{Instant, Duration};
use std::net::SocketAddr;
use quiche::*;
use ring::rand::*;
use std::net;
use crate::sender::{SenderDeque, BlockGenerator};

struct PartialResponse {
    body: Vec<u8>,

    written: usize,
}

struct Client {
    conn: quiche::Connection,

    partial_responses: HashMap<u64, PartialResponse>,
}

type ClientMap = HashMap<quiche::ConnectionId<'static>, Client>;

const MAX_DATAGRAM_SIZE: usize = 1350;
const HTTP_REQ_STREAM_ID: u64 = 64;
const IDLE_TIMEOUT: u64 = 5;

fn generate_cb(event: Instant, _metadata: &mut (), shared_data: &mut ServerGlobalData) -> TimeoutAction {
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
    trace!("Timeout for {:?} expired!", event);
    let next_gap = shared_data.block_generator.generate_once(&mut shared_data.sender_queue);

    // The timer event source requires us to return a TimeoutAction to
    // specify if the timer should be rescheduled. In our case we just drop it.
    if next_gap.is_some() {
        return TimeoutAction::ToDuration(std::time::Duration::from_secs_f32(next_gap.unwrap()));
    } else {
        // notify the event loop to stop running using the signal in the shared data
        // (see below)
        // TODO: remove when implement the whole system
        shared_data.signal.as_ref().unwrap().stop();
        return TimeoutAction::Drop;
    }
}

const USAGE: &str = "Usage:
server [options] CONFIG
server -h | --help
Options:
-h --help                Show this screen.
";
use dtp_utils::*;
#[derive(Default)]
struct ServerGlobalData<'a> {
    sender_queue: SenderDeque,
    block_generator: BlockGenerator,
    
    socket: Option<UdpSocket>,
    clients: Option<ClientMap>,
    conn_id_seed: Option<ring::hmac::Key>,
    local_addr: Option<SocketAddr>,
    config: Option<quiche::Config>,

    handle: Option<LoopHandle<'a, ServerGlobalData<'a>>>,
    timeout_dispatcher: Option<Dispatcher<'a, Timer, ServerGlobalData<'a>>>,
    timeout_token: Option<RegistrationToken>,
    signal: Option<LoopSignal>
}
#[derive(Default)]
struct ClientGlobalData<'b> {
    scid: Option<ConnectionId<'b>>,
    local_addr: Option<SocketAddr>,
    peer_addr: Option<SocketAddr>, 
    config: Option<quiche::Config>,
    req_start: Option<Instant>,
    req_sent: bool,


    conn: Option<quiche::Connection>,
    socket: Option<UdpSocket>,

    handle: Option<LoopHandle<'b, ClientGlobalData<'b>>>,
    timeout_dispatcher: Option<Dispatcher<'b, Timer, ClientGlobalData<'b>>>,
    timeout_token: Option<RegistrationToken>,
    signal: Option<LoopSignal>
}

fn server_recv_cb(
    _readiness: calloop::Readiness, 
    io_object: &mut UdpSocket, 
    shared_data: &mut ServerGlobalData
) -> Result<calloop::PostAction, std::io::Error>{
    // The first argument of the callback is a Readiness
    // The second is a &mut reference to your object
    let clients = shared_data.clients.as_mut().unwrap();
    let socket = io_object;
    let conn_id_seed = shared_data.conn_id_seed.as_ref().unwrap();
    let local_addr = shared_data.local_addr.as_ref().unwrap();
    let config = shared_data.config.as_mut().unwrap();

    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];

    'read: loop {
        // If the event loop reported no events, it means that the timeout
        // has expired, so handle it without attempting to read packets. We
        // will then proceed with the send loop.
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(v) => v,

            Err(e) => {
                // There are no more UDP packets to read, so end the read
                // loop.
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    debug!("recv() would block");
                    break 'read;
                }

                panic!("recv() failed: {:?}", e);
            },
        };

        debug!("server got {} bytes", len);

        let pkt_buf = &mut buf[..len];

        // Parse the QUIC packet's header.
        let hdr = match quiche::Header::from_slice(
            pkt_buf,
            quiche::MAX_CONN_ID_LEN,
        ) {
            Ok(v) => v,

            Err(e) => {
                error!("Parsing packet header failed: {:?}", e);
                continue 'read;
            },
        };

        trace!("server got packet {:?}", hdr);

        let conn_id = ring::hmac::sign(&conn_id_seed, &hdr.dcid);
        let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
        let conn_id = conn_id.to_vec().into();

        // Lookup a connection based on the packet's connection ID. If there
        // is no connection matching, create a new one.
        let client = if !clients.contains_key(&hdr.dcid) &&
            !clients.contains_key(&conn_id)
        {
            if hdr.ty != quiche::Type::Initial {
                error!("Packet is not Initial");
                continue 'read;
            }

            if !quiche::version_is_supported(hdr.version) {
                warn!("Doing version negotiation");

                let len =
                    quiche::negotiate_version(&hdr.scid, &hdr.dcid, &mut out)
                        .unwrap();

                let out = &out[..len];

                if let Err(e) = socket.send_to(out, from) {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                        break;
                    }

                    panic!("send() failed: {:?}", e);
                }
                continue 'read;
            }

            let mut scid = [0; quiche::MAX_CONN_ID_LEN];
            scid.copy_from_slice(&conn_id);

            let scid = quiche::ConnectionId::from_ref(&scid);

            // Token is always present in Initial packets.
            let token = hdr.token.as_ref().unwrap();

            // Do stateless retry if the client didn't send a token.
            if token.is_empty() {
                warn!("Doing stateless retry");

                let new_token = mint_token(&hdr, &from);

                let len = quiche::retry(
                    &hdr.scid,
                    &hdr.dcid,
                    &scid,
                    &new_token,
                    hdr.version,
                    &mut out,
                )
                .unwrap();

                let out = &out[..len];

                if let Err(e) = socket.send_to(out, from) {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                        break;
                    }

                    panic!("send() failed: {:?}", e);
                }
                continue 'read;
            }

            let odcid = validate_token(&from, token);

            // The token was not valid, meaning the retry failed, so
            // drop the packet.
            if odcid.is_none() {
                error!("Invalid address validation token");
                continue 'read;
            }

            if scid.len() != hdr.dcid.len() {
                error!("Invalid destination connection ID");
                continue 'read;
            }

            // Reuse the source connection ID we sent in the Retry packet,
            // instead of changing it again.
            let scid = hdr.dcid.clone();

            debug!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

            let conn = quiche::accept(
                &scid,
                odcid.as_ref(),
                *local_addr,
                from,
                config,
            )
            .unwrap();

            let client = Client {
                conn,
                partial_responses: HashMap::new(),
            };

            clients.insert(scid.clone(), client);

            clients.get_mut(&scid).unwrap()
        } else {
            match clients.get_mut(&hdr.dcid) {
                Some(v) => v,

                None => clients.get_mut(&conn_id).unwrap(),
            }
        };

        let recv_info = quiche::RecvInfo {
            to: socket.local_addr().unwrap(),
            from,
        };

        // Process potentially coalesced packets.
        let read = match client.conn.recv(pkt_buf, recv_info) {
            Ok(v) => v,

            Err(e) => {
                error!("{} recv failed: {:?}", client.conn.trace_id(), e);
                continue 'read;
            },
        };

        debug!("{} processed {} bytes", client.conn.trace_id(), read);

        if client.conn.is_in_early_data() || client.conn.is_established() {
            // Handle writable streams.
            for stream_id in client.conn.writable() {
                handle_writable(client, stream_id);
            }

            // Process all readable streams.
            for s in client.conn.readable() {
                while let Ok((read, fin)) =
                    client.conn.stream_recv(s, &mut buf)
                {
                    debug!(
                        "{} received {} bytes",
                        client.conn.trace_id(),
                        read
                    );

                    let stream_buf = &buf[..read];

                    debug!(
                        "{} stream {} has {} bytes (fin? {})",
                        client.conn.trace_id(),
                        s,
                        stream_buf.len(),
                        fin
                    );

                    handle_stream(client, s, stream_buf, "examples/root");
                }
            }
        }
    }

    collect_garbage_connection(clients);

    server_flush_quic_packets(clients, socket).unwrap();

    let timer_dispatcher= shared_data.timeout_dispatcher.as_ref().unwrap();
    let handle = shared_data.handle.as_ref().unwrap();
    let timer_token = shared_data.timeout_token.as_ref().unwrap();
    update_timer_outside_cb(clients, timer_dispatcher, handle, timer_token);
    
    // your callback needs to return a Result<PostAction, std::io::Error>
    // if it returns an error, the event loop will consider this event
    // event source as erroring and report it to the user.
    Ok(PostAction::Continue)
}

/// Generate outgoing QUIC packets for all active connections and send
/// them on the UDP socket, until quiche reports that there are no more
/// packets to be sent.
fn server_flush_quic_packets(clients: &mut ClientMap, socket: &mut UdpSocket) -> Result<()> {
    let mut out = [0; MAX_DATAGRAM_SIZE];
    for client in clients.values_mut() {
        loop {
            let (write, send_info) = match client.conn.send(&mut out) {
                Ok(v) => v,

                Err(quiche::Error::Done) => {
                    debug!("{} done writing", client.conn.trace_id());
                    break;
                },

                Err(e) => {
                    error!("{} send failed: {:?}", client.conn.trace_id(), e);

                    client.conn.close(false, 0x1, b"fail").ok();
                    break;
                },
            };

            if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    debug!("send() would block");
                    break;
                }

                panic!("send() failed: {:?}", e);
            }

            debug!("{} written {} bytes", client.conn.trace_id(), write);
        }
    }
    Ok(())
}

// Garbage collect closed connections.
fn collect_garbage_connection(clients: &mut ClientMap) {
    clients.retain(|_, ref mut c| {
        debug!("Server Collecting garbage");

        if c.conn.is_closed() {
            info!(
                "{} connection collected {:?}",
                c.conn.trace_id(),
                c.conn.stats()
            );
        }

        !c.conn.is_closed()
    });
}

/// update timer dispatcher and register it
fn update_timer_outside_cb(
    clients: &mut ClientMap, 
    timer_dispatcher: &Dispatcher<Timer, ServerGlobalData>, 
    handle: &LoopHandle<ServerGlobalData>, 
    timer_token: &RegistrationToken
) {
    let mut timer = timer_dispatcher.as_source_mut();
    if let Some(next_timeout) = clients.values().filter_map(|c| c.conn.timeout()).min() {
        timer.set_duration(next_timeout);
        handle.update(timer_token).unwrap();
    } else {
        debug!("recv packet but all timeout is None, set the timeout as timer idle timeout");
        timer.set_duration(Duration::from_secs(IDLE_TIMEOUT));
        handle.update(timer_token).unwrap();
    }
}

fn server_timeout_cb(
    _event: Instant, 
    _metadata: &mut (), 
    shared_data: &mut ServerGlobalData
) -> TimeoutAction {
    let clients = shared_data.clients.as_mut().unwrap();
    let socket = shared_data.socket.as_mut().unwrap();
    // handle timeout
    clients.values_mut().for_each(|c| c.conn.on_timeout());

    collect_garbage_connection(clients);

    server_flush_quic_packets(clients, socket).unwrap();

    // update timer
    if let Some(next_timeout ) = 
        clients.values().filter_map(|c| c.conn.timeout()).min() {
        TimeoutAction::ToDuration(next_timeout)
    } else {
        if clients.len() == 0 {
            debug!("all timeout is None and no client in timeout_cb, stop the evloop");
            shared_data.signal.as_ref().unwrap().stop();
        }
        TimeoutAction::Drop
    }
}

fn init_server(addr: SocketAddr, cfg_path: &str) -> Result<()> {
    // init global data
    let mut global_data= ServerGlobalData::default();
    // init socket
    let server_addr = addr;
    let server_socket = UdpSocket::bind(server_addr)?;
    let local_addr = server_socket.local_addr().unwrap();
    server_socket.set_nonblocking(true);
    global_data.local_addr = Some(local_addr);
    global_data.socket = Some(server_socket.try_clone().unwrap());
    // init config file
    let cfgs = get_dtp_config(cfg_path);
    if cfgs.len() == 0 {
        return Err(anyhow!("No configs in the file or filename error"));
    }
    global_data.block_generator.load_cfgs(cfgs);
    // init quiche
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    set_quiche_conn_config(&mut config);
    global_data.config = Some(config);

    // init random seed
    let rng = SystemRandom::new();
    let conn_id_seed =
        ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();
    global_data.conn_id_seed = Some(conn_id_seed);

    // init client map
    let clients = ClientMap::new();
    global_data.clients = Some(clients);

    // init eventloop
    // search for crate calloop for more information

    let mut event_loop: EventLoop<ServerGlobalData> =
        EventLoop::try_new().expect("Failed to initialize the event loop!");

    // Retrieve a handle. It is used to insert new sources into the event loop
    // It can be cloned, allowing you to insert sources from within source
    // callbacks.
    let handle = event_loop.handle();

    // Create our event source, a timer, that will expire in 2 seconds
    // let source = Timer::from_duration(std::time::Duration::from_secs_f32(global_data.block_generator.first_time_gap().unwrap()));

    // Inserting an event source takes this general form. It can also be done
    // from within the callback of another event source.
    // add block generator_cb
    // handle
    //     .insert_source(
    //         // a type which implements the EventSource trait
    //         source,
    //         // a callback that is invoked whenever this source generates an event
    //         generate_cb,
    //     )
    //     .expect("Failed to insert generate_cb!");
    // TODO: add sender_cb
    // add recv_cb
    handle.insert_source(
        // wrap your IO object in a Generic, here we register for read readiness
        // in level-triggering mode
        Generic::new(server_socket, Interest::READ, Mode::Level),
        server_recv_cb
    ).expect("Failed to insert recv_cb!");
    // add timeout cb
    let timeout_timer = Timer::from_duration(std::time::Duration::from_secs(IDLE_TIMEOUT));

    let timeout_dispatcher = Dispatcher::new(timeout_timer, server_timeout_cb);

    let timeout_token = handle.register_dispatcher(timeout_dispatcher.clone()).expect("Failed to insert timeout");
 
    global_data.handle = Some(handle);
    global_data.timeout_dispatcher = Some(timeout_dispatcher);
    global_data.timeout_token = Some(timeout_token);
    global_data.signal = Some(event_loop.get_signal());

    // Create the shared data for our loop.
    let mut shared_data = global_data;

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
            std::time::Duration::from_secs(10),
            &mut shared_data,
            |_shared_data| {
                // Finally, this is where you can insert the processing you need
                // to do do between each waiting event eg. drawing logic if
                // you're doing a GUI app.
            },
        )
        .expect("Error during event loop!");
    Ok(())
}

// Generate outgoing QUIC packets and send them on the UDP socket, until
// quiche reports that there are no more packets to be sent.
fn client_recv_cb(
    _readiness: calloop::Readiness, 
    _io_object: &mut UdpSocket, 
    shared_data: &mut ClientGlobalData 
) -> Result<calloop::PostAction, std::io::Error>{
    let mut buf = [0; 65535];
    let socket = shared_data.socket.as_mut().unwrap();
    let conn = shared_data.conn.as_mut().unwrap();
    let req_sent = &mut shared_data.req_sent;
    let req_start = shared_data.req_start.as_ref().unwrap();
    let peer_addr = shared_data.peer_addr.as_ref().unwrap();
    // Read incoming UDP packets from the socket and feed them to quiche,
    // until there are no more packets to read.
    'read: loop {
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(v) => v,

            Err(e) => {
                // There are no more UDP packets to read, so end the read
                // loop.
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    debug!("client recv() would block");
                    break 'read;
                }

                panic!("client recv() failed: {:?}", e);
            },
        };

        debug!("client got {} bytes", len);

        let recv_info = quiche::RecvInfo {
            to: socket.local_addr().unwrap(),
            from,
        };

        // Process potentially coalesced packets.
        let read = match conn.recv(&mut buf[..len], recv_info) {
            Ok(v) => v,

            Err(e) => {
                error!("recv failed: {:?}", e);
                continue 'read;
            },
        };

        debug!("client processed {} bytes", read);
    }

    debug!("client done reading");

    if conn.is_closed() {
        info!("client connection closed, {:?}", conn.stats());
        return Ok(PostAction::Continue);
    }

    // Send an HTTP request as soon as the connection is established.
    if conn.is_established() && !*req_sent {
        info!("sending HTTP request for {:?}", peer_addr.to_string());

        let req = format!("GET {}\r\n", "Hello world");
        conn.stream_send(HTTP_REQ_STREAM_ID, req.as_bytes(), true)
            .unwrap();

        *req_sent = true;
    }

    // Process all readable streams.
    for s in conn.readable() {
        while let Ok((read, fin)) = conn.stream_recv(s, &mut buf) {
            debug!("client received {} bytes", read);

            let stream_buf = &buf[..read];

            debug!(
                "client stream {} has {} bytes (fin? {})",
                s,
                stream_buf.len(),
                fin
            );

            print!("{}", unsafe {
                std::str::from_utf8_unchecked(stream_buf)
            });

            // The server reported that it has no more data to send, which
            // we got the full response. Close the connection.
            if s == HTTP_REQ_STREAM_ID && fin {
                info!(
                    "client response received in {:?}, closing...",
                    req_start.elapsed()
                );

                conn.close(true, 0x00, b"kthxbye").unwrap();
            }
        }
    }

    client_flush_quic_packets(socket, conn).unwrap();

    if conn.is_closed() {
        info!("connection closed, {:?}", conn.stats());
        shared_data.signal.as_ref().unwrap().stop();
        return Ok(PostAction::Remove);
    }
    Ok(PostAction::Continue)
}

fn client_flush_quic_packets(socket: &mut UdpSocket, conn: &mut quiche::Connection) -> Result<()> {
    let mut out = [0; MAX_DATAGRAM_SIZE];
    loop {
        let (write, send_info) = match conn.send(&mut out) {
            Ok(v) => v,

            Err(quiche::Error::Done) => {
                debug!("client done writing");
                break;
            },

            Err(e) => {
                error!("client send failed: {:?}", e);

                conn.close(false, 0x1, b"fail").ok();
                break;
            },
        };

        if let Err(e) = socket.send_to(&out[..write], send_info.to) {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                debug!("send() would block");
                break;
            }

            panic!("send() failed: {:?}", e);
        }

        debug!("client written {}", write);
    }
    Ok(())
}

fn client_timeout_cb(
    _event: Instant, 
    _metadata: &mut (), 
    shared_data: &mut ClientGlobalData
) -> TimeoutAction {
    let socket = shared_data.socket.as_mut().unwrap();
    let conn = shared_data.conn.as_mut().unwrap();
    // handle timeout
    conn.on_timeout();

    if conn.is_closed() {
        info!("connection closed, {:?}", conn.stats());
        shared_data.signal.as_ref().unwrap().stop();
        return TimeoutAction::Drop;
    }

    client_flush_quic_packets(socket, conn).unwrap();

    // update timer
    if let Some(next_timeout) = conn.timeout() {
        TimeoutAction::ToDuration(next_timeout)
    } else {
        TimeoutAction::Drop
    }
}

fn init_client(addr: SocketAddr, peer_addr: SocketAddr) -> Result<()> {
    // init global data
    let mut global_data= ClientGlobalData::default();
    // init socket
    let client_addr = addr;
    let client_socket = UdpSocket::bind(client_addr)?;
    client_socket.set_nonblocking(true)?;
    let local_addr = client_socket.local_addr().unwrap();
    global_data.local_addr = Some(local_addr);
    global_data.socket = Some(client_socket.try_clone().unwrap());
    global_data.peer_addr = Some(peer_addr);
    // init quiche
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    set_quiche_conn_config(&mut config);
    global_data.config = Some(config);

    // Generate a random source connection ID for the connection.
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    SystemRandom::new().fill(&mut scid[..]).unwrap();

    let scid = quiche::ConnectionId::from_ref(&scid);
    global_data.scid = Some(scid);

    // TODO: init data structure

    // init eventloop
    // search for crate calloop for more information

    let mut event_loop: EventLoop<ClientGlobalData> =
        EventLoop::try_new().expect("Failed to initialize the client event loop!");

    // Retrieve a handle. It is used to insert new sources into the event loop
    // It can be cloned, allowing you to insert sources from within source
    // callbacks.
    let handle = event_loop.handle();

    // Create our event source, a timer, that will expire in 2 seconds
    let source = Timer::from_duration(std::time::Duration::from_secs_f32(2.0));

    // Inserting an event source takes this general form. It can also be done
    // from within the callback of another event source.
    // add block generator_cb
    handle
        .insert_source(
            // a type which implements the EventSource trait
            source,
            // a callback that is invoked whenever this source generates an event
            |_event: Instant, _metadata: &mut (), shared_data: &mut ClientGlobalData| {
                let mut out = [0; MAX_DATAGRAM_SIZE];
                let config = shared_data.config.as_mut().unwrap();
                let socket = shared_data.socket.as_mut().unwrap();
                let scid = shared_data.scid.as_ref().unwrap();
                // Create a QUIC connection and initiate handshake.
                let mut conn =
                    quiche::connect(Some(peer_addr.to_string().as_str()), &scid, local_addr, peer_addr, config)
                        .unwrap();

                info!(
                    "connecting to {:} from {:} with scid {}",
                    peer_addr,
                    socket.local_addr().unwrap(),
                    hex_dump(&scid)
                );

                let (write, send_info) = conn.send(&mut out).expect("initial send failed");

                while let Err(e) = socket.send_to(&out[..write], send_info.to) {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                        continue;
                    }

                    panic!("send() failed: {:?}", e);
                }

                debug!("written {}", write);

                shared_data.req_start = Some(std::time::Instant::now());

                shared_data.req_sent = false;

                shared_data.conn = Some(conn);
                return TimeoutAction::Drop
        }
    ).expect("Failed to insert generate_cb!");
    // TODO: add sender_cb
    // add recv_cb
    handle.insert_source(
        // wrap your IO object in a Generic, here we register for read readiness
        // in level-triggering mode
        Generic::new(client_socket, Interest::READ, Mode::Level),
        client_recv_cb
    ).expect("Failed to insert recv_cb!");
    // add timeout cb
    let timeout_timer = Timer::from_duration(std::time::Duration::from_secs(IDLE_TIMEOUT));

    let timeout_dispatcher = Dispatcher::new(timeout_timer, client_timeout_cb);

    let timeout_token = handle.register_dispatcher(timeout_dispatcher.clone()).expect("Failed to insert timeout");
 
    global_data.handle = Some(handle);
    global_data.timeout_dispatcher = Some(timeout_dispatcher);
    global_data.timeout_token = Some(timeout_token);
    global_data.signal = Some(event_loop.get_signal());

    // Create the shared data for our loop.
    let mut shared_data = global_data;

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
            std::time::Duration::from_secs(10),
            &mut shared_data,
            |_shared_data| {
                // Finally, this is where you can insert the processing you need
                // to do do between each waiting event eg. drawing logic if
                // you're doing a GUI app.
            },
        )
        .expect("Error during event loop!");
    Ok(())
}

fn main() -> Result<()> {
    // parse args
    let args = docopt::Docopt::new(USAGE)
        .and_then(|dopt| dopt.parse())
        .unwrap_or_else(|e| e.exit());
    // init logger
    env_logger::init();
    let cfg_path = args.get_str("CONFIG").to_owned();
    // let cfg_path = "aitrans_block.txt";

    let server_addr = SocketAddr::from(([127, 0, 0, 1], 7736));
    let client_addr = SocketAddr::from(([127, 0, 0, 1], 8847));

    use std::thread;
    let server_handle = thread::spawn(move ||{
        init_server(server_addr, cfg_path.as_str()).unwrap();
    });
    let client_handle = thread::spawn(move ||{
        init_client(client_addr, server_addr).unwrap();
    });

    server_handle.join().expect("The server thread has panicked");
    client_handle.join().expect("The client thread has panicked");
    
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something() {

    }
}

fn set_quiche_conn_config(config: &mut Config) {
    config
        .load_cert_chain_from_pem_file("cert.crt")
        .unwrap();
    config
        .load_priv_key_from_pem_file("cert.key")
        .unwrap();

    config
        .set_application_protos(&[
            b"hq-interop",
            b"hq-29",
            b"hq-28",
            b"hq-27",
            b"http/0.9",
        ])
        .unwrap();

    config.set_max_idle_timeout(IDLE_TIMEOUT * 1000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_stream_data_uni(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);
    config.enable_early_data();
}

/// Generate a stateless retry token.
///
/// The token includes the static string `"quiche"` followed by the IP address
/// of the client and by the original destination connection ID generated by the
/// client.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn mint_token(hdr: &quiche::Header, src: &net::SocketAddr) -> Vec<u8> {
    let mut token = Vec::new();

    token.extend_from_slice(b"quiche");

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    token.extend_from_slice(&addr);
    token.extend_from_slice(&hdr.dcid);

    token
}

/// Validates a stateless retry token.
///
/// This checks that the ticket includes the `"quiche"` static string, and that
/// the client IP address matches the address stored in the ticket.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn validate_token<'a>(
    src: &net::SocketAddr, token: &'a [u8],
) -> Option<quiche::ConnectionId<'a>> {
    if token.len() < 6 {
        return None;
    }

    if &token[..6] != b"quiche" {
        return None;
    }

    let token = &token[6..];

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    if token.len() < addr.len() || &token[..addr.len()] != addr.as_slice() {
        return None;
    }

    Some(quiche::ConnectionId::from_ref(&token[addr.len()..]))
}

/// Handles newly writable streams.
fn handle_writable(client: &mut Client, stream_id: u64) {
    let conn = &mut client.conn;

    debug!("{} stream {} is writable", conn.trace_id(), stream_id);

    if !client.partial_responses.contains_key(&stream_id) {
        return;
    }

    let resp = client.partial_responses.get_mut(&stream_id).unwrap();
    let body = &resp.body[resp.written..];

    let written = match conn.stream_send(stream_id, body, true) {
        Ok(v) => v,

        Err(quiche::Error::Done) => 0,

        Err(e) => {
            client.partial_responses.remove(&stream_id);

            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    };

    resp.written += written;

    if resp.written == resp.body.len() {
        client.partial_responses.remove(&stream_id);
    }
}

/// Handles incoming HTTP/0.9 requests.
fn handle_stream(client: &mut Client, stream_id: u64, buf: &[u8], _root: &str) {
    // pass
    info!("server recv client: {}, stream_id: {}, buf: {}", client.conn.trace_id(), stream_id, String::from_utf8_lossy(&buf));
}

fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{b:02x}")).collect();

    vec.join("")
}

mod block;
mod sender;