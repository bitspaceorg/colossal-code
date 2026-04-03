use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("missing sandbox probe command");
        std::process::exit(2);
    };

    let result = match command.as_str() {
        "read-file" => {
            let path = PathBuf::from(expect_arg(&mut args, "path"));
            fs::read_to_string(path).map(|contents| {
                print!("{contents}");
            })
        }
        "write-file" => {
            let path = PathBuf::from(expect_arg(&mut args, "path"));
            let contents = expect_arg(&mut args, "contents");
            fs::write(path, contents).map(|_| {
                println!("ok");
            })
        }
        "http-get" => {
            let url = expect_arg(&mut args, "url");
            http_get(&url).map(|body| {
                print!("{body}");
            })
        }
        _ => {
            eprintln!("unknown sandbox probe command: {command}");
            std::process::exit(2);
        }
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn expect_arg(args: &mut impl Iterator<Item = String>, name: &str) -> String {
    args.next().unwrap_or_else(|| {
        eprintln!("missing sandbox probe argument: {name}");
        std::process::exit(2);
    })
}

fn http_get(url: &str) -> std::io::Result<String> {
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = TcpStream::connect((host.as_str(), port))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (_, body) = response.split_once("\r\n\r\n").unwrap_or(("", ""));
    Ok(body.to_string())
}

fn parse_http_url(url: &str) -> std::io::Result<(String, u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported URL scheme: {url}"),
        )
    })?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, suffix)) => (authority, format!("/{suffix}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("invalid URL port: {err}"),
                )
            })?;
            (host.to_string(), port)
        }
        None => (authority.to_string(), 80),
    };

    Ok((host, port, path))
}
