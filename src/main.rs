use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::{Body, Bytes};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, Method, StatusCode};
use hyper_util::rt::TokioIo;
use musicbrainz_rs::entity::artist_credit::ArtistCredit;
use sqlx::Database;
use tokio::net::TcpListener;
use build_html::*;

use musicbrainz_rs::prelude::*;
use musicbrainz_rs::entity::release_group::*; use sqlx::{SqlitePool, pool::PoolConnection, Sqlite};

trait Query {
    fn to_tree() -> BTreeMap<String, String>;
    fn to_string() -> String;
}

fn query_to_map(query: &str) -> BTreeMap<&str, &str> {
    let splits: Vec<&str> = query.split("&").collect();
    let mut map = BTreeMap::new();
    for s in splits {
        let kvpair: Vec<&str> = s.split("=").collect();
        map.insert(kvpair[0], kvpair[1]);
    }
    map
}

fn map_to_query(map: &BTreeMap<&str, &str>) -> String {
    let mut s = String::from("");
    for (key, value) in map {
        s += format!("{}={}&", key, value).as_str();
    }
    s.pop();
    s
}

fn simple_page_response(content: String) -> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::new(
        Full::new(Bytes::from(
            HtmlPage::new()
                .with_title("simple page")
                .with_container(
                    Container::new(ContainerType::Div)
                        .with_paragraph(content)
                )
                .to_html_string()
        ))
    ))
}
async fn mid_addition(request: Request<hyper::body::Incoming>) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let map = query_to_map(request.uri().query().unwrap());
    let offset: &str = match map.get("offset") {
        Some(val) => val,
        None => "0"
    };
    let result_iterator: Vec<ReleaseGroup> = ReleaseGroup::search(
        ReleaseGroupSearchQuery::query_builder().release_group(map["name"]).build() + &format!("&offset={}", offset).to_string()
    ).execute().await.unwrap().entities;

    let mut c = Container::new(ContainerType::Div);
    for r in result_iterator {
        c.add_link(format!("/add-final?name={}&mbid={}", r.title, r.id), format!("{} - {}",    r.title,
            r.artist_credit.unwrap()[0].name
        ));
        c.add_raw("<br>");
    }

    map.insert("offset", (offset.parse::<i32>().unwrap() + 25).to_string());
    if map.contains_key("offset") {
        let mut map = map.clone();
        
        c.add_link(format!("{}", request.uri().path_and_query().unwrap()), "Next");
    }
    else {
        c.add_link(format!("{}&offset={}", request.uri().path_and_query().unwrap(), 25), "Next");
    }

    Ok(Response::new(
       Full::new(Bytes::from(
           HtmlPage::new()
                .with_title("simple page")
                .with_container(
                    c
                )
                .to_html_string()
            )
        )
    ))
}

async fn add_final(request: Request<hyper::body::Incoming>, sql: &SqlitePool) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let map = query_to_map(request.uri().query().unwrap());
    
    if !map.contains_key("name") || !map.contains_key("mbid") {
        return simple_page_response("no name or mbid".to_string());
    }

    sqlx::query("insert into album values(NULL, ?, ?);")
        .bind(urlencoding::decode(map["name"]).expect("UTF-8"))
        .bind(map["mbid"])
        .execute(sql).await.unwrap();
    
    let val: Vec<(i32, String, String)> = sqlx::query_as("select * from album;")
        .bind(query_to_map(request.uri().query().unwrap())["name"])
        .fetch_all(sql).await.unwrap();

    simple_page_response(format!("{:?}", val))
}

async fn add(_request: Request<hyper::body::Incoming>) 
-> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::new(
        Full::new(Bytes::from(
            HtmlPage::new()
                .with_title("simple page")
                .with_container(
                    Container::new(ContainerType::Div)
                    .with_raw("
                    <form method=\"get\" action=\"/mid-addition\">
                        <label for=\"name\">Album name</label><br>
                        <input type=\"text\" id=\"name\" name=\"name\">
                        <input type=\"submit\" value=\"Submit\">
                    </form>
                    ")
                )
                .to_html_string()
        ))
    ))
}
 

async fn hello(request: Request<hyper::body::Incoming>) 
-> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::new(
        Full::new(Bytes::from(
            HtmlPage::new()
                .with_title(format!("{}!", request.uri()))
                .with_style(
                     "p {
                         color: blue;
                     }"
                )
                .with_container(
                    Container::new(ContainerType::Div)
                        .with_paragraph(format!("hello world, look {}", request.uri()))
                )
                .to_html_string()
        ))
    ))
}

async fn router(request: Request<hyper::body::Incoming>, sql: &SqlitePool) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let path: Vec<&str> = request.uri().path().split("/").collect();
    match path[1] {
        "add" => add(request).await,
        "mid-addition" => mid_addition(request).await,
        "add-final" => add_final(request, sql).await,
        "hello" => hello(request).await,
        _ => {
            let mut res = Response::new(Full::new(Bytes::from("")));
            *res.status_mut() = StatusCode::NOT_FOUND;
            Ok(res)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    let pool = SqlitePool::connect("db.db").await.unwrap();

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);
        let pool2 = pool.clone();

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(|request| {
                    router(request, &pool2)
                }))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

