use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io::{Read, prelude::*};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use build_html::*;

use musicbrainz_rs::prelude::*;
use musicbrainz_rs::entity::release_group::*; 
use sqlx::SqlitePool;

fn query_to_map(query: &str) -> BTreeMap<String, String> {
    let splits: Vec<&str> = query.split("&").collect();
    let mut map = BTreeMap::new();
    for s in splits {
        let kvpair: Vec<&str> = s.split("=").collect();
        map.insert(kvpair[0].to_string(), kvpair[1].to_string());
    }
    map
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

fn linked_album_image(rg: &ReleaseGroup) -> String {
    format!("<a href=\"/album?{0}\" title=\"{1} - {2}\"><img src=\"/image?{0}\" alt=\"{1}\"></img></a>", rg.id, rg.title, get_artist(rg.clone()))
}

async fn mid_addition(request: Request<hyper::body::Incoming>) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let query = MidAdditionQuery::from_query(request.uri().query().unwrap()).unwrap();
    let result_iterator: Vec<ReleaseGroup> = ReleaseGroup::search(
        ReleaseGroupSearchQuery::query_builder().release_group(&query.name).build() + &format!("&offset={}", query.offset).to_string()
    ).execute().await.unwrap().entities;

    let mut c = Container::new(ContainerType::Div);
    for rg in result_iterator {
        c.add_raw(format!("<a href=\"/add-final?name={1}&mbid={0}\" title=\"{1} - {2}\"><img src=\"/image?{0}\" alt=\"{1}\"></img></a>", rg.id, rg.title, get_artist(rg.clone())));
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

    let val: Vec<(String, String)> = sqlx::query_as("select * from album;")
        .fetch_all(sql).await.unwrap();

    match sqlx::query("insert into album values(?, ?);")
        .bind(query.mbid)
        .bind(query.name)
        .execute(sql).await {
        Ok(_) => {
            simple_page_response(format!("{:?}", val))
        },
        Err(_) => {
            let res = simple_page_response(format!("{:?}", val));
            let mut res = res.unwrap();
                *res.status_mut() = StatusCode::BAD_REQUEST;
            Ok(res)
        }
    }
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

fn album_info(rg: ReleaseGroup) -> Container {
    let mut genres_string = String::new();
    println!("{:?}", rg);
    let genres = rg.genres.clone().unwrap();
    println!("{:?}", genres);
    for i in genres {
        genres_string.push_str(format!("{}, ", i.name).as_str());
        println!("{}", i.name);
    }

    genres_string.pop();
    genres_string.pop();

    Container::new(ContainerType::Div)
    .with_paragraph(format!("Title: {}", rg.title))
    .with_paragraph(format!("Artist : {}", rg.clone().artist_credit.unwrap()[0].name))
    .with_paragraph(format!("Genres: {}", genres_string))
    .with_paragraph(format!("Debug Info: {:?}", rg))
}

async fn album(request: Request<hyper::body::Incoming>, release_map: &ReleaseMap)
-> Result<Response<Full<Bytes>>, Infallible> {
    let rg = get_release_group(&request.uri().query().unwrap().to_string(), release_map).await;

    let mut c = Container::new(ContainerType::Div);
    c.add_raw(linked_album_image(&rg));
    c.add_container(album_info(rg.clone()));
    Ok(Response::new(Full::new(Bytes::from(HtmlPage::new().with_container(c).to_html_string()))))
}
 
async fn image(request: Request<hyper::body::Incoming>)
-> Result<Response<Full<Bytes>>, Infallible> {
    let id = request.uri().query().unwrap();

    std::fs::create_dir_all("cached_covers").unwrap();
    Ok(Response::new(Full::new(
        match std::fs::File::open(format!("cached_covers/{}", id)) {
            Ok(mut f) => {
                println!("reading from file");
                let mut vec = Vec::new();
                f.read_to_end(&mut vec).unwrap();
                Bytes::from(vec)
            },
            Err(_) => {
                println!("reading from site");
                let cover_art = ReleaseGroup::fetch().id(id).execute().await.unwrap()
                    .get_coverart().front().res_500().execute().await.unwrap();

                let cover_art = match cover_art {
                    musicbrainz_rs::entity::CoverartResponse::Json(c) => {
                        c.images[0].image.clone()
                    },
                    musicbrainz_rs::entity::CoverartResponse::Url(url) => {
                        url
                    }
                };

                let response = reqwest::get(cover_art).await.unwrap().bytes().await.unwrap();
                let mut f = std::fs::File::create(format!("cached_covers/{}", id)).unwrap();
                f.write_all(&response).unwrap();
                response
            }
        }
    )))
}

async fn get_release_group(id: &String, release_map: &ReleaseMap)
-> ReleaseGroup {
    let mut map = release_map.lock().await; 
    if map.contains_key(id) {
        return map[id].clone()
    } else {
        let rg = ReleaseGroup::fetch().id(id.as_str())
            .with_artists()
            .with_tags()
            .with_genres()
            .with_aliases()
            .with_ratings()
            .with_releases()
            .with_series_relations()
            .with_release_group_relations()
            .execute().await.unwrap();
        map.insert(id.clone(), rg.clone()); 
        rg
    }
}

fn get_artist(r: ReleaseGroup) -> String {
    match &r.artist_credit {
        Some(v) => {
            match v.get(0) {
                Some(a) => {
                    a.name.clone()
                },
                None => "Artist Unknown EmptyVec".to_string()
            }
        },
        None => "Artist Unknown Credit".to_string()
    }
}

async fn home(_request: Request<hyper::body::Incoming>, sql: &SqlitePool, release_map: &ReleaseMap) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let val: Vec<(String, String)> = sqlx::query_as("select * from album;")
        .fetch_all(sql).await.unwrap();
    
    let mut release_groups: Vec<ReleaseGroup> = Vec::new();
    for i in val {
        release_groups.push(get_release_group(&i.0, release_map).await);
    }
   
    let mut c = Container::new(ContainerType::Div);

    for r in release_groups {
        c.add_raw(format!("<a href=\"/album?{0}\" title=\"{1} - {2}\"><img src=\"/image?{0}\" alt=\"{1}\"></img></a>", r.id, r.title, get_artist(r.clone())));
       c.add_raw("<br>");
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

type ReleaseMap = Arc<Mutex<BTreeMap<String, ReleaseGroup>>>;

async fn router(request: Request<hyper::body::Incoming>, sql: &SqlitePool, release_map: &ReleaseMap) 
-> Result<Response<Full<Bytes>>, Infallible> {
    let path: Vec<&str> = request.uri().path().split("/").collect();
    match path[1] {
        "add" => add(request).await,
        "mid-addition" => mid_addition(request).await,
        "add-final" => add_final(request, sql).await,
        "image" => image(request).await,
        "album" => album(request, release_map).await,
        "" => home(request, sql, release_map).await,
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
    let listener = TcpListener::bind(addr).await?;
    let pool = SqlitePool::connect("db.db").await.unwrap();

    let release_map = Arc::new(Mutex::new(BTreeMap::<String, ReleaseGroup>::new()));

    loop {
        let (stream, _) = listener.accept().await?;

        let io = TokioIo::new(stream);
        let pool2 = pool.clone();
        let release_map2 = release_map.clone();

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(|request| {
                    router(request, &pool2, &release_map2)
                }))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

