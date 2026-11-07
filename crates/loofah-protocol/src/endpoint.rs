pub fn validate_endpoint(
    url: &url::Url,
    allow_insecure_loopback: bool,
) -> Result<(), &'static str> {
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || (allow_insecure_loopback && url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Server and identity endpoints must use HTTPS");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_policy_preserves_literal_loopback_identity_and_url_semantics() {
        for (address, insecure, accepted) in [
            ("https://capture.example/", false, true),
            ("https://capture.example/path?query=1", false, true),
            ("https://localhost/", false, true),
            ("http://127.0.0.1/", false, false),
            ("http://127.0.0.1:8765/path?query=1", true, true),
            ("http://127.42.0.9/", true, true),
            ("http://[::1]:8765/", true, true),
            ("http://localhost/", true, false),
            ("http://capture.example/", true, false),
            ("http://192.168.1.10/", true, false),
            ("https://user@capture.example/", false, false),
            ("https://user:password@capture.example/", false, false),
            ("https://capture.example/#fragment", false, false),
            ("file:///capture", true, false),
        ] {
            let result = validate_endpoint(&url::Url::parse(address).unwrap(), insecure);
            assert_eq!(result.is_ok(), accepted, "{address}, insecure={insecure}");
            if !accepted {
                assert_eq!(
                    result.unwrap_err(),
                    "Server and identity endpoints must use HTTPS"
                );
            }
        }
    }
}
