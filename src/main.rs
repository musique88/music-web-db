use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::{Body, Bytes};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, Method, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use build_html::*;

use musicbrainz_rs::prelude::*;
use musicbrainz_rs::entity::release_group::*; use sqlx::{SqlitePool, pool::PoolConnection, Sqlite};

fn query_to_map(query: &str) -> BTreeMap<String, String> {
    let splits: Vec<&str> = query.split("&").collect();
    let mut map = BTreeMap::new();
    for s in splits {
        let kvpair: Vec<&str> = s.split("=").collect();
        map.insert(kvpair[0].to_string(), kvpair[1].to_string());
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

fn url_decode(url: &String) -> String {
    String::from(urlencoding::decode(url).expect("UTF-8"))
}

fn url_encode(string: &String) -> String {
    String::from(urlencoding::encode(string))
}

trait Query {
    fn to_tree(&self) -> BTreeMap<String, String>;
    fn to_string(&self) -> String;
    fn from_query(query: &str) -> Option<Self> where Self: Sized;
    fn from_map(map: BTreeMap<String, String>) -> Option<Self> where Self: Sized;
}

struct MidAdditionQuery {
    name: String,
    offset: i32,
}

impl Query for MidAdditionQuery {
    fn to_tree(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), self.name.clone());
        if self.offset != 0 {
            map.insert("offset".to_string(), self.offset.to_string());
        }
        map
    }

    fn to_string(&self) -> String {
        if self.offset == 0 {
            format!("name={}", url_encode(&self.name))
        } else {
            format!("name={}&offset={}", url_encode(&self.name), self.offset)
        }
    }

    fn from_query(query: &str) -> Option<Self> where Self: Sized {
        Self::from_map(query_to_map(query))
    }

    fn from_map(map: BTreeMap<String, String>) -> Option<Self> where Self: Sized {
        if !map.contains_key("name") {
            return None;
        }
        
        Some(Self{
            name: url_decode(&map["name"]), 
            offset: if map.contains_key("offset") {map["offset"].parse::<i32>().unwrap()} else {0}})
    }
}

struct FinalAddition {
    name: String,
    mbid: String,
}

impl Query for FinalAddition {
    fn to_tree(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), self.name.clone());
        map.insert("mbid".to_string(), self.mbid.clone());
        map
    }

    fn to_string(&self) -> String {
        format!("name={}&mbid={}", url_encode(&self.name), url_encode(&self.mbid))
    }

    fn from_query(query: &str) -> Option<Self> where Self: Sized {
        Self::from_map(query_to_map(query))
    }

    fn from_map(map: BTreeMap<String, String>) -> Option<Self> where Self: Sized {
        if !map.contains_key("name") || !map.contains_key("mbid") {
            return None;
        }
        
        Some(Self{name: url_decode(&map["name"]), mbid: url_decode(&map["mbid"])})
    }
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
    let query = MidAdditionQuery::from_query(request.uri().query().unwrap()).unwrap();
    let result_iterator: Vec<ReleaseGroup> = ReleaseGroup::search(
        ReleaseGroupSearchQuery::query_builder().release_group(&query.name).build() + &format!("&offset={}", query.offset).to_string()
    ).execute().await.unwrap().entities;

    let mut c = Container::new(ContainerType::Div);
    for r in result_iterator {
        c.add_link(format!("/add-final?name={}&mbid={}", r.title, r.id), format!("{} - {}",    r.title,
            r.artist_credit.unwrap()[0].name
        ));
        c.add_raw("<br>");
    }

    if query.offset != 0 {
        c.add_link(format!("/mid-addition?name={}&offset={}", query.name, query.offset - 25), "Prev");
    }
    c.add_link(format!("/mid-addition?name={}&offset={}", query.name, query.offset + 25), "Next");

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
    let query = FinalAddition::from_query(request.uri().query().unwrap()).unwrap();

    sqlx::query("insert into album values(NULL, ?, ?);")
        .bind(query.name)
        .bind(query.mbid)
        .execute(sql).await.unwrap();
    
    let val: Vec<(i32, String, String)> = sqlx::query_as("select * from album;")
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

async fn image(request: Request<hyper::body::Incoming>)
-> Result<Response<Full<Bytes>>, Infallible> {
    let cover_art = ReleaseGroup::fetch().id(request.uri().query().unwrap()).execute().await.unwrap()
        .get_coverart().execute().await.unwrap();

    let cover_art = match cover_art {
        musicbrainz_rs::entity::CoverartResponse::Json(c) => {
            c.images[0].image.clone()
        },
        musicbrainz_rs::entity::CoverartResponse::Url(url) => {
            url
        }
    };

    Ok(Response::new(Full::new(reqwest::get(cover_art).await.unwrap().bytes().await.unwrap())))
}

async fn router(request: Request<hyper::body::Incoming>, sql: &SqlitePool) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let path: Vec<&str> = request.uri().path().split("/").collect();
    match path[1] {
        "add" => add(request).await,
        "mid-addition" => mid_addition(request).await,
        "add-final" => add_final(request, sql).await,
        "image" => image(request).await,
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

