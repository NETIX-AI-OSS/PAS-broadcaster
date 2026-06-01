use std::net::Ipv4Addr;

pub fn parse_admin_multicast(value: &str) -> Result<Ipv4Addr, String> {
    let addr: Ipv4Addr = value
        .parse()
        .map_err(|_| format!("'{value}' is not a valid IPv4 address"))?;

    if !addr.is_multicast() {
        return Err(format!("{addr} is not an IPv4 multicast address"));
    }

    if addr.octets()[0] != 239 {
        return Err(format!(
            "{addr} is multicast, but not in the administratively scoped 239.0.0.0/8 range"
        ));
    }

    Ok(addr)
}

pub fn validate_port(port: u16) -> Result<(), String> {
    if port == 0 {
        Err("port must be greater than 0".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_admin_scoped_multicast() {
        assert_eq!(
            parse_admin_multicast("239.10.10.1").unwrap(),
            Ipv4Addr::new(239, 10, 10, 1)
        );
    }

    #[test]
    fn rejects_non_multicast() {
        assert!(parse_admin_multicast("192.168.1.10").is_err());
    }

    #[test]
    fn rejects_global_multicast() {
        assert!(parse_admin_multicast("224.0.0.1").is_err());
    }
}
