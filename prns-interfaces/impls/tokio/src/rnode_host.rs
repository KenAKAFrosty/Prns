use std::io;

use prns_config::RNodeTransportPlan;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::rnode::{
    RNodeDetectTimeout, RNodeKeepalive, BLE_RNODE_DETECT_TIMEOUT, DEFAULT_RNODE_DETECT_TIMEOUT,
    TCP_RNODE_DETECT_TIMEOUT, TCP_RNODE_KEEPALIVE,
};
use crate::serial_host::open_host_serial;
use crate::tcp::tokio_socket::{connect, tune, TcpConnectionSettings};

pub(crate) const RNODE_BAUD: u32 = 115_200;

pub(crate) trait RNodeHostStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> RNodeHostStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub(crate) type BoxedRNodeHostStream = Box<dyn RNodeHostStream>;

#[derive(Clone)]
pub(crate) struct RNodeHostOpener {
    transport: RNodeTransportPlan,
}

impl RNodeHostOpener {
    pub(crate) const fn new(transport: RNodeTransportPlan) -> Self {
        Self { transport }
    }

    pub(crate) const fn detect_timeout(&self) -> RNodeDetectTimeout {
        match &self.transport {
            RNodeTransportPlan::Serial(_) => DEFAULT_RNODE_DETECT_TIMEOUT,
            RNodeTransportPlan::Tcp(_) => TCP_RNODE_DETECT_TIMEOUT,
            RNodeTransportPlan::Ble(_) => BLE_RNODE_DETECT_TIMEOUT,
        }
    }

    pub(crate) const fn keepalive(&self) -> RNodeKeepalive {
        match &self.transport {
            RNodeTransportPlan::Serial(_) => RNodeKeepalive::Disabled,
            RNodeTransportPlan::Tcp(_) => TCP_RNODE_KEEPALIVE,
            RNodeTransportPlan::Ble(_) => RNodeKeepalive::Disabled,
        }
    }

    pub(crate) async fn open(&self) -> io::Result<BoxedRNodeHostStream> {
        match &self.transport {
            RNodeTransportPlan::Serial(device) => {
                let stream = open_host_serial(device.as_str(), RNODE_BAUD)?;
                Ok(Box::new(stream))
            }
            RNodeTransportPlan::Tcp(target) => {
                let stream = open_tcp(&target.socket_target()).await?;
                Ok(Box::new(stream))
            }
            RNodeTransportPlan::Ble(target) => {
                let stream = crate::rnode_ble::open(target).await?;
                Ok(Box::new(stream))
            }
        }
    }
}

async fn open_tcp(target: &str) -> io::Result<tokio::net::TcpStream> {
    let stream = connect(target, TcpConnectionSettings::STOCK).await?;
    tune(&stream);
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_config::{parse_and_plan, PlannedMedium};
    use tokio::net::TcpListener;

    fn planned_transport(port: &str) -> RNodeTransportPlan {
        let config = format!(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = {port}\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\n"
        );
        let plan = parse_and_plan(&config).expect("RNode config plans").value;
        let PlannedMedium::Rnode { transport, .. } = &plan.interfaces[0].medium else {
            panic!("RNode transport expected")
        };
        transport.clone()
    }

    #[test]
    fn each_transport_selects_its_own_detect_and_keepalive_policy() {
        let serial = RNodeHostOpener::new(planned_transport("/dev/ttyUSB0"));
        assert_eq!(serial.detect_timeout(), DEFAULT_RNODE_DETECT_TIMEOUT);
        assert_eq!(serial.keepalive(), RNodeKeepalive::Disabled);

        let tcp = RNodeHostOpener::new(planned_transport("tcp://radio.example"));
        assert_eq!(tcp.detect_timeout(), TCP_RNODE_DETECT_TIMEOUT);
        assert_eq!(tcp.keepalive(), TCP_RNODE_KEEPALIVE);

        let ble = RNodeHostOpener::new(planned_transport("ble://RNode 1234"));
        assert_eq!(ble.detect_timeout(), BLE_RNODE_DETECT_TIMEOUT);
        assert_eq!(ble.keepalive(), RNodeKeepalive::Disabled);
    }

    #[tokio::test]
    async fn the_tcp_opener_connects_and_applies_the_shared_socket_discipline() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds a test RNode TCP endpoint");
        let address = listener.local_addr().expect("bound address");
        let target = address.to_string();
        let (opened, accepted) = tokio::join!(open_tcp(&target), listener.accept());
        let stream = opened.expect("RNode TCP connection opens");
        accepted.expect("RNode endpoint accepts the connection");
        assert!(stream.nodelay().expect("reads TCP_NODELAY"));
    }
}
