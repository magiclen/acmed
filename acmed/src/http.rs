use crate::acme_proto::structs::HttpApiError;
use crate::endpoint::Endpoint;
use acme_common::crypto::X509Certificate;
use acme_common::error::Error;
use attohttpc::{charsets, header, Response, Session};
use std::fs::File;
use std::io::prelude::*;
use std::{thread, time};

pub const CONTENT_TYPE_JOSE: &str = "application/jose+json";
pub const CONTENT_TYPE_JSON: &str = "application/json";
pub const CONTENT_TYPE_PEM: &str = "application/pem-certificate-chain";
pub const HEADER_NONCE: &str = "Replay-Nonce";
pub const HEADER_LOCATION: &str = "Location";

fn is_nonce(data: &str) -> bool {
    !data.is_empty()
        && data
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn new_nonce(endpoint: &mut Endpoint, root_certs: &[String]) -> Result<(), Error> {
    rate_limit(endpoint);
    let url = endpoint.dir.new_nonce.clone();
    let _ = get(endpoint, root_certs, &url)?;
    Ok(())
}

fn update_nonce(endpoint: &mut Endpoint, response: &Response) -> Result<(), Error> {
    if let Some(nonce) = response.headers().get(HEADER_NONCE) {
        let nonce = header_to_string(&nonce)?;
        if !is_nonce(&nonce) {
            let msg = format!("{}: invalid nonce.", &nonce);
            return Err(msg.into());
        }
        endpoint.nonce = Some(nonce);
    }
    Ok(())
}

fn check_status(response: &Response) -> Result<(), Error> {
    if !response.is_success() {
        let status = response.status();
        let msg = format!("HTTP error: {}: {}", status.as_u16(), status.as_str());
        return Err(msg.into());
    }
    Ok(())
}

fn rate_limit(endpoint: &mut Endpoint) {
    endpoint.rl.block_until_allowed();
}

pub fn header_to_string(header_value: &header::HeaderValue) -> Result<String, Error> {
    let s = header_value
        .to_str()
        .map_err(|_| Error::from("Invalid nonce format."))?;
    Ok(s.to_string())
}

fn get_session(root_certs: &[String]) -> Result<Session, Error> {
    let useragent = format!(
        "{}/{} ({}) {}",
        crate::APP_NAME,
        crate::APP_VERSION,
        env!("ACMED_TARGET"),
        env!("ACMED_HTTP_LIB_AGENT")
    );
    // TODO: allow to change the language
    let mut session = Session::new();
    session.default_charset(Some(charsets::UTF_8));
    session.try_header(header::ACCEPT_LANGUAGE, "en-US,en;q=0.5")?;
    session.try_header(header::USER_AGENT, &useragent)?;
    for crt_file in root_certs.iter() {
        let mut buff = Vec::new();
        File::open(crt_file)?.read_to_end(&mut buff)?;
        let crt = X509Certificate::from_pem_native(&buff)?;
        session.add_root_certificate(crt);
    }
    Ok(session)
}

pub fn get(endpoint: &mut Endpoint, root_certs: &[String], url: &str) -> Result<Response, Error> {
    let mut session = get_session(root_certs)?;
    session.try_header(header::ACCEPT, CONTENT_TYPE_JSON)?;
    rate_limit(endpoint);
    let response = session.get(url).send()?;
    update_nonce(endpoint, &response)?;
    check_status(&response)?;
    Ok(response)
}

pub fn post<F>(
    endpoint: &mut Endpoint,
    root_certs: &[String],
    url: &str,
    data_builder: &F,
    content_type: &str,
    accept: &str,
) -> Result<Response, Error>
where
    F: Fn(&str, &str) -> Result<String, Error>,
{
    let mut session = get_session(root_certs)?;
    session.try_header(header::ACCEPT, accept)?;
    session.try_header(header::CONTENT_TYPE, content_type)?;
    if endpoint.nonce.is_none() {
        let _ = new_nonce(endpoint, root_certs);
    }
    for _ in 0..crate::DEFAULT_HTTP_FAIL_NB_RETRY {
        let nonce = &endpoint.nonce.clone().unwrap();
        let body = data_builder(&nonce, url)?;
        rate_limit(endpoint);
        let response = session.post(url).text(&body).send()?;
        update_nonce(endpoint, &response)?;
        match check_status(&response) {
            Ok(_) => {
                return Ok(response);
            }
            Err(e) => {
                let api_err = response.json::<HttpApiError>()?;
                let acme_err = api_err.get_acme_type();
                if !acme_err.is_recoverable() {
                    return Err(e);
                }
            }
        }
        thread::sleep(time::Duration::from_secs(crate::DEFAULT_HTTP_FAIL_WAIT_SEC));
    }
    Err("Too much errors, will not retry".into())
}

pub fn post_jose<F>(
    endpoint: &mut Endpoint,
    root_certs: &[String],
    url: &str,
    data_builder: &F,
) -> Result<Response, Error>
where
    F: Fn(&str, &str) -> Result<String, Error>,
{
    post(
        endpoint,
        root_certs,
        url,
        data_builder,
        CONTENT_TYPE_JOSE,
        CONTENT_TYPE_JSON,
    )
}

#[cfg(test)]
mod tests {
    use super::is_nonce;

    #[test]
    fn test_nonce_valid() {
        let lst = [
            "XFHw3qcgFNZAdw",
            "XFHw3qcg-NZAdw",
            "XFHw3qcg_NZAdw",
            "XFHw3qcg-_ZAdw",
            "a",
            "1",
            "-",
            "_",
        ];
        for n in lst.iter() {
            assert!(is_nonce(n));
        }
    }

    #[test]
    fn test_nonce_invalid() {
        let lst = [
            "",
            "rdo9x8gS4K/mZg==",
            "rdo9x8gS4K/mZg",
            "rdo9x8gS4K+mZg",
            "৬",
            "京",
        ];
        for n in lst.iter() {
            assert!(!is_nonce(n));
        }
    }
}
