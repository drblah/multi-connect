use std::net::{IpAddr};
use std::os::fd::{AsRawFd, FromRawFd};
use anyhow::Result;
use smol::fs::{File};
use smol::io::{AsyncReadExt, AsyncWriteExt};
use tun::platform::Device;

pub struct AsyncTun {
    device: Device,
    file: File,
}

impl AsyncTun {
    pub fn new(tun_device_name: &str, ip: IpAddr, netmask: IpAddr) -> Result<Self> {
        let mut config = tun::Configuration::default();

        // TODO: Handle IPv6 TUN addresses on creation
        config
            .name(tun_device_name)
            .address(ip)
            .netmask(netmask)
            .up();

        let dev = tun::create(&config)?;

        let mut file = None;
        unsafe {
            file = Some(File::from_raw_fd(dev.as_raw_fd()));
        }

        if file.is_some() {
            let file = file.unwrap();

            let async_tun = AsyncTun{
                device: dev,
                file
            };

            return Ok(async_tun);
        }

        return Err(anyhow::anyhow!("Could not open tun device"));
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf).await
    }

    pub async fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf).await
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;
    use smol::io::AsyncReadExt;
    use crate::tun_device::open_tun;

    /*
    #[test]
    fn read_test() {
        smol::block_on(async {
            let tun_device = "tun0";
            let mut file = open_tun(tun_device).await.unwrap();

            let mut buffer = [0u8; 65535];

            let len = file.read(&mut buffer).await.unwrap();

            println!("{:?}", buffer[..len].to_vec());
        })
    }

     */
}