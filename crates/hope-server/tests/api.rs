use axum::{Router, body::Body, http::{Request, StatusCode}};
use hope_protocol::*;
use hope_server::{router, state::{State, hash, now}, agents, metrics};
use http_body_util::BodyExt;
use serde_json::{Value,json};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup()->(TempDir,State,Router,String){
    let dir=tempfile::tempdir().expect("temporary database");
    let state=State::open(dir.path().join("test.db").to_str().expect("path"),"http://localhost:8080".into(),"releases".into(),Capabilities{platform:"test".into(),discovery_mode:"limited".into(),arp:false,icmp:false,limitations:vec![]}).await.expect("state");
    state.set_meta("setup_token_hash",&hash("test-setup-token-long-enough")).await.expect("setup token");
    let app=router(state.clone(),"web/out");
    let response=app.clone().oneshot(Request::builder().method("POST").uri("/api/v1/auth/setup").header("content-type","application/json").body(Body::from(json!({"password":"a-long-test-password","setup_token":"test-setup-token-long-enough"}).to_string())).expect("request")).await.expect("response");
    assert_eq!(response.status(),StatusCode::OK);
    let cookie=response.headers()["set-cookie"].to_str().expect("cookie").split(';').next().expect("session").to_string();
    (dir,state,app,cookie)
}
async fn request(app:&Router,method:&str,path:&str,body:Value,credential:Option<(&str,&str)>)->(StatusCode,Value){
    let mut builder=Request::builder().method(method).uri(path).header("content-type","application/json");
    if let Some((name,value))=credential{builder=builder.header(name,value);}
    let response=app.clone().oneshot(builder.body(Body::from(body.to_string())).expect("request")).await.expect("response");
    let status=response.status();let bytes=response.into_body().collect().await.expect("body").to_bytes();
    (status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
fn inventory()->Inventory{Inventory{hostname:"test-host".into(),os:"Linux test".into(),architecture:"aarch64".into(),cpu_model:"Test CPU".into(),cpu_cores:4,memory_total_bytes:8e9,interfaces:vec![Interface{name:"eth0".into(),mac:Some("02:00:00:00:00:01".into()),addresses:vec!["192.168.1.10".into()],gateway:Some("192.168.1.1".into())}],disks:vec![],gpus:vec![],collectors:vec![]}}
async fn enrolled(state:&State,app:&Router)->(String,String,String){
    let token=agents::issue_token(state).await.expect("token");let agent=Uuid::new_v4().to_string();
    let (status,body)=request(app,"POST","/api/v1/agent/enroll",json!(EnrollmentRequest{schema_version:1,token:token.clone(),agent_id:agent.clone(),inventory:inventory()}),None).await;
    assert_eq!(status,StatusCode::OK,"{body}");
    (agent,body["credential"].as_str().expect("credential").into(),token)
}
#[tokio::test]async fn authentication_setup_and_cross_site_protection(){
    let (_dir,state,app,cookie)=setup().await;
    assert_eq!(request(&app,"GET","/api/v1/bootstrap",Value::Null,None).await.0,StatusCode::UNAUTHORIZED);
    assert_eq!(request(&app,"GET","/api/v1/bootstrap",Value::Null,Some(("cookie",&cookie))).await.0,StatusCode::OK);
    assert!(state.meta("setup_token_hash").await.expect("meta").is_none());
    assert_eq!(request(&app,"POST","/api/v1/auth/setup",json!({"password":"another-password","setup_token":"test-setup-token-long-enough"}),None).await.0,StatusCode::BAD_REQUEST);
    let response=app.oneshot(Request::builder().method("POST").uri("/api/v1/networks").header("origin","https://evil.example").header("cookie",cookie).header("content-type","application/json").body(Body::from(json!({"name":"LAN","cidr":"192.168.1.0/24"}).to_string())).expect("request")).await.expect("response");
    assert_eq!(response.status(),StatusCode::FORBIDDEN);
}
#[tokio::test]async fn token_is_single_use_and_expired_token_cannot_enroll(){
    let (_dir,state,app,_)=setup().await;
    let (_,_,token)=enrolled(&state,&app).await;
    let input=EnrollmentRequest{schema_version:1,token,agent_id:Uuid::new_v4().to_string(),inventory:inventory()};
    assert_eq!(request(&app,"POST","/api/v1/agent/enroll",json!(input),None).await.0,StatusCode::UNAUTHORIZED);
    let token=agents::issue_token(&state).await.expect("token");
    sqlx::query("UPDATE enrollment_tokens SET expires_at=0 WHERE token_hash=?").bind(hash(&token)).execute(&state.0.db).await.expect("expire token");
    assert_eq!(request(&app,"POST","/api/v1/agent/enroll",json!(EnrollmentRequest{schema_version:1,token,agent_id:Uuid::new_v4().to_string(),inventory:inventory()}),None).await.0,StatusCode::UNAUTHORIZED);
}
#[tokio::test]async fn reporting_deduplicates_and_keeps_identity_after_dhcp_and_rejects_revocation(){
    let (_dir,state,app,cookie)=setup().await;
    let (agent,credential,_)=enrolled(&state,&app).await;let bearer=format!("Bearer {credential}");
    let host_id=state.hosts().await.expect("hosts")[0].id.clone();
    let mut inv=inventory();inv.interfaces[0].addresses=vec!["192.168.1.22".into()];
    let mut report=AgentReport{schema_version:1,report_id:Uuid::new_v4().to_string(),agent_id:agent.clone(),collected_at:now(),inventory:Some(inv),metrics:Metrics{cpu_percent:Some(42.0),..Default::default()},collectors:vec![]};
    assert_eq!(request(&app,"POST","/api/v1/agent/report",json!(report),Some(("authorization",&bearer))).await.0,StatusCode::OK);
    let duplicate=request(&app,"POST","/api/v1/agent/report",json!(report),Some(("authorization",&bearer))).await;
    assert_eq!(duplicate.1["duplicate"],true);
    let hosts=state.hosts().await.expect("hosts");assert_eq!(hosts.len(),1);assert_eq!(hosts[0].id,host_id);assert_eq!(hosts[0].addresses,vec!["192.168.1.22"]);
    report.report_id=Uuid::new_v4().to_string();report.collected_at-=300.0;report.metrics.cpu_percent=Some(10.0);
    assert_eq!(request(&app,"POST","/api/v1/agent/report",json!(report),Some(("authorization",&bearer))).await.0,StatusCode::OK);
    assert_eq!(state.host(&host_id).await.expect("host").expect("exists").metrics.expect("metrics").cpu_percent,Some(42.0));
    let history=request(&app,"GET",&format!("/api/v1/hosts/{host_id}/metrics?hours=1"),Value::Null,Some(("cookie",&cookie))).await;
    assert_eq!(history.1.as_array().expect("history").len(),2);
    request(&app,"POST",&format!("/api/v1/agents/{agent}/revoke"),json!({}),Some(("cookie",&cookie))).await;
    assert_eq!(request(&app,"POST","/api/v1/agent/report",json!(report),Some(("authorization",&bearer))).await.0,StatusCode::UNAUTHORIZED);
}
#[tokio::test]async fn retention_averages_old_samples_and_deletes_expired_history(){
    let (_dir,state,_app,_)=setup().await;let old=((now()-90000.0)/300.0).floor()*300.0;
    for (id,time,cpu) in [("one",old+10.0,Some(10.0)),("two",old+40.0,Some(30.0)),("missing",old+70.0,None),("expired",now()-8.0*86400.0,Some(90.0))]{
        sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent',?,?,?)").bind(id).bind(time).bind(serde_json::to_string(&Metrics{cpu_percent:cpu,..Default::default()}).expect("json")).execute(&state.0.db).await.expect("insert");
    }
    metrics::retain(&state).await.expect("retention");
    let rows:Vec<(i64,String)>=sqlx::query_as("SELECT resolution,body FROM metrics").fetch_all(&state.0.db).await.expect("metrics");
    assert_eq!(rows.len(),1);assert_eq!(rows[0].0,300);assert_eq!(serde_json::from_str::<Metrics>(&rows[0].1).expect("metrics").cpu_percent,Some(20.0));
}
#[tokio::test]async fn network_validation_and_restart_recovery(){
    let (dir,state,app,cookie)=setup().await;
    for cidr in ["0.0.0.0/0","192.168.0.0/16","not-a-cidr","224.0.0.0/24"]{
        assert_eq!(request(&app,"POST","/api/v1/networks",json!({"name":"Bad","cidr":cidr}),Some(("cookie",&cookie))).await.0,StatusCode::BAD_REQUEST);
    }
    assert_eq!(request(&app,"POST","/api/v1/networks",json!({"name":"LAN","cidr":"192.168.1.12/24"}),Some(("cookie",&cookie))).await.0,StatusCode::OK);
    assert_eq!(request(&app,"POST","/api/v1/networks",json!({"name":"Duplicate","cidr":"192.168.1.0/24"}),Some(("cookie",&cookie))).await.0,StatusCode::BAD_REQUEST);
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('interrupted','net','192.168.1.0/24','running',254,?)").bind(now()).execute(&state.0.db).await.expect("scan");
    let reopened=State::open(dir.path().join("test.db").to_str().expect("path"),"http://localhost:8080".into(),"releases".into(),state.0.capabilities.clone()).await.expect("reopen");
    let status:String=sqlx::query_scalar("SELECT status FROM scans WHERE id='interrupted'").fetch_one(&reopened.0.db).await.expect("status");assert_eq!(status,"interrupted");
}
