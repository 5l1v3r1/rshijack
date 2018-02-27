use pnet::transport::{TransportSender, TransportReceiver, transport_channel};
use pnet::transport::TransportChannelType::Layer3;
use pnet::packet::tcp::MutableTcpPacket;
use pnet::packet::ipv4::{MutableIpv4Packet, Ipv4Flags};
use pnet::packet::ip::IpNextHeaderProtocols;
use pnet::packet::MutablePacket;

pub use pnet::packet::tcp::{TcpFlags, ipv4_checksum};


use pcap::Capture;
use nom::IResult::Done;
use pktparse::ethernet;
use pktparse::ipv4;
use pktparse::tcp;

use std::net::IpAddr;
use std::net::SocketAddrV4;

use ::Result;


pub fn getseqack(interface: &str, src: &SocketAddrV4, dst: &SocketAddrV4) -> Result<(u32, u32, usize)> {
    let mut cap = Capture::from_device(interface)?
                .open()?;

    while let Ok(packet) = cap.next() {
        trace!("received {:?}", packet);

        if let Done(remaining, eth_frame) = ethernet::parse_ethernet_frame(&packet.data) {
            debug!("eth: {:?}", eth_frame);

            match eth_frame.ethertype {
                ethernet::EtherType::IPv4 => {
                    if let Done(remaining, ip_hdr) = ipv4::parse_ipv4_header(remaining) {
                        debug!("ip4: {:?}", ip_hdr);

                        // skip packet if src/dst ip doesn't match
                        if *src.ip() != ip_hdr.source_addr ||
                           *dst.ip() != ip_hdr.dest_addr {
                               continue;
                        }

                        match ip_hdr.protocol {
                            ipv4::IPv4Protocol::TCP => {

                                if let Done(remaining, tcp_hdr) = tcp::parse_tcp_header(remaining) {
                                    debug!("tcp: {:?}", tcp_hdr);

                                    // skip packet if src/dst port doesn't match
                                    if src.port() != tcp_hdr.source_port ||
                                       dst.port() != tcp_hdr.dest_port {
                                           continue;
                                    }

                                    // skip packet if ack flag not set
                                    if !tcp_hdr.flag_ack {
                                            continue;
                                    }

                                    return Ok((
                                        tcp_hdr.sequence_no,
                                        tcp_hdr.ack_no,
                                        remaining.len(),
                                    ));
                                }
                            },
                            _ => (),
                        }
                    }
                },
                _ => (),
            }
        }
    }

    Err("Reading from interface failed!".into())
}

pub fn create_socket() -> Result<(TransportSender, TransportReceiver)> {
    let protocol = Layer3(IpNextHeaderProtocols::Tcp);
    let (tx, rx) = transport_channel(4096, protocol)?;
    Ok((tx, rx))
}

pub fn sendtcp(tx: &mut TransportSender, src: &SocketAddrV4, dst: &SocketAddrV4, flags: u16, seq: u32, ack: u32, data: &[u8]) -> Result<()> {
    let tcp_len = MutableTcpPacket::minimum_packet_size() + data.len();
    let total_len = MutableIpv4Packet::minimum_packet_size() + tcp_len;

    let mut pkt_buf: Vec<u8> = vec![0; total_len];

    // populate ipv4
    let ipv4_header_len = match MutableIpv4Packet::minimum_packet_size().checked_div(4) {
        Some(l) => l as u8,
        None => bail!("Invalid header len")
    };

    let mut ipv4 = MutableIpv4Packet::new(&mut pkt_buf).unwrap();
    ipv4.set_header_length(ipv4_header_len);
    ipv4.set_total_length(total_len as u16);

    ipv4.set_next_level_protocol(IpNextHeaderProtocols::Tcp);
    ipv4.set_source(src.ip().to_owned());
    ipv4.set_version(4);
    ipv4.set_ttl(64);
    ipv4.set_destination(dst.ip().clone());
    ipv4.set_flags(Ipv4Flags::DontFragment);
    ipv4.set_options(&[]);

    // populate tcp
    {
        let mut tcp = MutableTcpPacket::new(ipv4.payload_mut()).unwrap();

        let tcp_header_len = match MutableTcpPacket::minimum_packet_size().checked_div(4) {
            Some(l) => l as u8,
            None => bail!("Invalid header len")
        };
        tcp.set_data_offset(tcp_header_len);

        tcp.set_source(src.port());
        tcp.set_destination(dst.port());
        tcp.set_sequence(seq);
        tcp.set_acknowledgement(ack);
        tcp.set_flags(flags);
        tcp.set_window(data.len() as u16);

        tcp.set_payload(data);

        let chk = ipv4_checksum(&tcp.to_immutable(), src.ip().clone(), dst.ip().clone());
        tcp.set_checksum(chk);
    };

    match tx.send_to(ipv4, IpAddr::V4(dst.ip().clone())) {
        Ok(bytes) => if bytes != total_len { bail!("short send count: {}", bytes) },
        Err(e) => bail!("Could not send: {}", e),
    };

    Ok(())
}
