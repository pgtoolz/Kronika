//! Bounded traffic between two sockets on the namespace loopback interface.

use super::{wait_for, waveform};
use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DATAGRAM_BYTES: usize = 8 * 1024;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(1);

fn connected_pair() -> Result<(UdpSocket, UdpSocket)> {
    let receiver = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("bind the system workload loopback receiver")?;
    let sender = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("bind the system workload loopback sender")?;
    let receiver_address = receiver
        .local_addr()
        .context("read the loopback receiver address")?;
    let sender_address = sender
        .local_addr()
        .context("read the loopback sender address")?;
    anyhow::ensure!(
        receiver_address.ip().is_loopback() && sender_address.ip().is_loopback(),
        "the system workload sockets are not bound to loopback"
    );
    sender
        .connect(receiver_address)
        .context("connect the loopback sender")?;
    receiver
        .connect(sender_address)
        .context("connect the loopback receiver")?;
    sender
        .set_write_timeout(Some(SOCKET_TIMEOUT))
        .context("set the loopback write timeout")?;
    receiver
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .context("set the loopback read timeout")?;
    Ok((sender, receiver))
}

fn transfer(
    sender: &UdpSocket,
    receiver: &UdpSocket,
    bytes: u64,
    payload: &[u8],
    receive_buffer: &mut [u8],
) -> Result<(u64, u64)> {
    let mut remaining = bytes;
    let mut sent_total = 0_u64;
    let mut received_total = 0_u64;
    let payload_len = u64::try_from(payload.len()).context("loopback buffer size exceeds u64")?;
    while remaining > 0 {
        let length = usize::try_from(remaining.min(payload_len))
            .context("loopback datagram size exceeds usize")?;
        let sent = sender
            .send(&payload[..length])
            .context("send system workload loopback traffic")?;
        anyhow::ensure!(
            sent == length,
            "the loopback sender wrote a partial datagram"
        );
        let read = receiver
            .recv(receive_buffer)
            .context("receive system workload loopback traffic")?;
        anyhow::ensure!(
            read == length,
            "the loopback receiver read an unexpected datagram size"
        );
        let transferred = u64::try_from(length).context("loopback datagram size exceeds u64")?;
        remaining -= transferred;
        sent_total += transferred;
        received_total += transferred;
    }
    Ok((sent_total, received_total))
}

pub(super) fn run(peak_kib_per_s: u64, stop: &Arc<AtomicBool>) -> Result<()> {
    let (sender, receive_socket) = connected_pair()?;
    let payload = vec![0x4b_u8; DATAGRAM_BYTES];
    let mut receive_buffer = vec![0_u8; DATAGRAM_BYTES];
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let tick_started = Instant::now();
        let bytes = waveform::payload_bytes_for_tick(peak_kib_per_s, started.elapsed());
        transfer(
            &sender,
            &receive_socket,
            bytes,
            &payload,
            &mut receive_buffer,
        )?;
        let rest = waveform::WORKER_TICK.saturating_sub(tick_started.elapsed());
        if wait_for(stop, rest) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
