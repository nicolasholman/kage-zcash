// Loopback gRPC transport fixtures; verification, decryption, receipts and SQLite are production code.
// Tx bytes are genuinely encrypted/proved in-process using the existing verify_real_tx builder.
mod audit_tx { include!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../audit/exec/fac-transaction.rs")); }
#[derive(Clone)]
struct Replay { data:Vec<u8>,height:u64,tip:u64,time:u32,mode:String,chain:String,calls:u32 }
type ReplayState=Arc<std::sync::Mutex<Replay>>;
macro_rules! audit_unary {
 ($name:ident,$req:ty,$resp:ty,$handle:expr)=>{
 struct $name(ReplayState);
 impl tonic::server::UnaryService<$req> for $name {
 type Response=$resp;type Future=std::pin::Pin<Box<dyn std::future::Future<Output=Result<tonic::Response<$resp>,tonic::Status>>+Send>>;
 fn call(&mut self,_:tonic::Request<$req>)->Self::Future{let mut state=self.0.lock().unwrap();state.calls+=1;let r:Result<$resp,tonic::Status>=($handle)(&*state);Box::pin(async move{r.map(tonic::Response::new)})}
 }
 };
}
use zcash_client_backend::proto::{service as grpc,compact_formats::CompactBlock};
audit_unary!(AuditTip,grpc::ChainSpec,grpc::BlockId,|s:&Replay|if s.mode=="tip-failure"{Err(tonic::Status::unavailable("audit tip outage"))}else{Ok(grpc::BlockId{height:s.tip,hash:vec![]})});
audit_unary!(AuditTx,grpc::TxFilter,grpc::RawTransaction,|s:&Replay|if s.mode=="not-found"{Err(tonic::Status::not_found("audit unknown transaction"))}else if s.mode=="transaction-failure"{Err(tonic::Status::unavailable("audit transaction outage"))}else{Ok(grpc::RawTransaction{data:s.data.clone(),height:s.height})});
audit_unary!(AuditBlock,grpc::BlockId,CompactBlock,|s:&Replay|if s.mode=="block-failure"{Err(tonic::Status::unavailable("audit block time outage"))}else{Ok(CompactBlock{height:s.height,time:s.time,..Default::default()})});
audit_unary!(AuditInfo,grpc::Empty,grpc::LightdInfo,|s:&Replay|Ok(grpc::LightdInfo{chain_name:s.chain.clone(),block_height:s.tip,..Default::default()}));
struct ReplayGuard(tokio::task::JoinHandle<()>);impl Drop for ReplayGuard{fn drop(&mut self){self.0.abort();}}
async fn audit_grpc_listener(s:ReplayState)->(String,ReplayGuard){let l=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=l.local_addr().unwrap();assert!(addr.port()>4142);let mut router=Router::new();
    macro_rules! route {($path:literal,$svc:ident)=>{{let s=s.clone();router=router.route(concat!("/cash.z.wallet.sdk.rpc.CompactTxStreamer/",$path),post(move|req:axum::extract::Request|{let s=s.clone();async move{let mut grpc=tonic::server::Grpc::new(tonic_prost::ProstCodec::default());grpc.unary($svc(s),req).await.into_response()}}));}};}
    route!("GetLatestBlock",AuditTip);route!("GetTransaction",AuditTx);route!("GetBlock",AuditBlock);route!("GetLightdInfo",AuditInfo);
    let guard=ReplayGuard(tokio::spawn(async move{axum::serve(l,router).await.unwrap();}));(format!("http://{addr}"),guard)
}
fn audit_binding_count(path:&std::path::Path)->u64{rusqlite::Connection::open(path).unwrap().query_row("SELECT (SELECT count(*) FROM issued)+(SELECT count(*) FROM bindings)",[],|r|r.get(0)).unwrap()}
fn audit_db_path()->PathBuf{static N:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);std::env::temp_dir().join(format!("kage-exec-fac-{}-{}.sqlite",std::process::id(),N.fetch_add(1,std::sync::atomic::Ordering::SeqCst)))}
fn audit_cleanup_db(path:&std::path::Path){for ext in ["","-wal","-shm"]{let _=std::fs::remove_file(format!("{}{ext}",path.display()));}}
async fn audit_replay_app(ufvk:&zcash_keys::keys::UnifiedFullViewingKey,bytes:Vec<u8>)->(Arc<App>,ReplayState,ReplayGuard,PathBuf){let h=u64::from(u32::from(audit_tx::target_height()));let s=Arc::new(std::sync::Mutex::new(Replay{data:bytes,height:h,tip:h,time:1792560000,mode:String::new(),chain:"test".into(),calls:0}));let(url,guard)=audit_grpc_listener(s.clone()).await;let mut a=app();let path=audit_db_path();{let a=Arc::get_mut(&mut a).unwrap();a.key=audit_tx::uivk_key(ufvk);a.lwd_url=url.clone();a.lwd=lwd::connect(&url).await.unwrap();a.issued=IssuedStore::open(&path).unwrap();}(a,s,guard,path)}
fn audit_wire(ch:&PaymentRequired,txid:&str)->Vec<u8>{body(PaymentPayload::zcash(txid,&ch.memo_nonce),PaymentRequirements::from_challenge(ch,"http://localhost/premium","d","application/json",900))}
fn audit_txid(bytes:&[u8])->String{let h=audit_tx::target_height();verify::parse_tx(&NETWORK,bytes,Some(h),h).unwrap().txid().to_string()}
#[tokio::test]
async fn audit_grpc_real_transaction_route_failures(){
    use audit_tx::*;let ufvk=merchant(1);let ua=ua_at(&ufvk,0);let pay_to=ua.encode(&NETWORK);let t=zcash_transparent::address::TransparentAddress::PublicKeyHash([0x11;20]);
    let valid=build_tx(&[Out::Ironwood(*ua.orchard().unwrap(),100000,NONCE)]);let under=build_tx(&[Out::Ironwood(*ua.orchard().unwrap(),99999,NONCE)]);let topup=build_tx(&[Out::Transparent(t,100000),Out::Ironwood(*ua.orchard().unwrap(),99999,NONCE)]);let transparent=build_tx(&[Out::Transparent(t,100000)]);
    for settle in [false,true]{for (offset,mode,why,error,code)in [
        (174,"underpaid","invalid_payment","underpaid: 99999 zat < 100000 zat",422),
        (175,"nonce","invalid_payment","no output in this transaction decrypts to the merchant viewing key with this memo nonce",422),
        (176,"key","invalid_payment","no output in this transaction decrypts to the merchant viewing key with this memo nonce",422),
        (178,"recipient","invalid_payment","memo matches but the note was sent to a different address than payTo",422),
        (179,"transparent-address","invalid_payment","payTo is not a shielded-only testnet unified address",422),
        (180,"transparent-only","invalid_payment","no output in this transaction decrypts",422),
        (181,"topup","invalid_payment","underpaid: 99999 zat < 100000 zat",422),
        (183,"expired","payment_expired","after the challenge expired",422),
        (184,"garbage","invalid_payment","transaction did not parse:",422),
        (185,"txid","invalid_payment","lightwalletd returned tx",422),
        (187,"block-failure","facilitator_error","block ",502),
        (189,"not-found","payment_not_found","lightwalletd does not know tx",404),
        (190,"confirmations","settlement_pending","need 2",202),
        (191,"orphan","settlement_pending","only mined on a fork; wait",202)
    ]{
        let bytes=match mode{"underpaid"=>under.clone(),"topup"=>topup.clone(),"transparent-only"=>transparent.clone(),_=>valid.clone()};let txid=audit_txid(&bytes);let(mut a,s,_guard,path)=audit_replay_app(&ufvk,bytes).await;let mut ch=challenge(&pay_to,100000);let asked=if mode=="txid"{"ab".repeat(32)}else{txid.clone()};
        match mode{"nonce"=>ch.memo_nonce="x402z-another".into(),"key"=>Arc::get_mut(&mut a).unwrap().key=uivk_key(&merchant(2)),"recipient"=>ch.pay_to=ua_at(&ufvk,7).encode(&NETWORK),"transparent-address"=>ch.pay_to=zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().copied(),ua.sapling().copied(),Some(t),None,None).unwrap().encode(&NETWORK),"expired"=>ch.expires_at=1792550000,"garbage"=>s.lock().unwrap().data=vec![0;40],"block-failure"|"not-found"=>s.lock().unwrap().mode=mode.into(),"confirmations"=>Arc::get_mut(&mut a).unwrap().min_confirmations=2,"orphan"=>s.lock().unwrap().height=u64::MAX,_=>{}}
        let r=audit_request(a.clone(),settle,audit_headers(),audit_wire(&ch,&asked)).await;assert_eq!(r.status(),code,"{mode}");let v=audit_json(r).await;assert_eq!(v[if settle{"success"}else{"isValid"}],false,"{mode}");assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],why,"{mode}");if !settle{assert_eq!(v["payer"],"");}assert!(v["error"].as_str().unwrap().contains(error),"{mode}: unexpected diagnostic");assert!(v.get("receipt").is_none());assert_eq!(audit_binding_count(&path),0);if mode=="confirmations"{assert_eq!(v["status"],"pending");if settle{assert_eq!(v["confirmations"],1);assert_eq!(v["minConfirmations"],2);}}if mode=="not-found"{assert_eq!(v["status"],"not_found");}audit_mark(if settle{offset+29}else{offset});drop(a);audit_cleanup_db(&path);
    }}
}
#[tokio::test]
async fn audit_grpc_success_replay_database_faults_and_health(){
    use audit_tx::*;let ufvk=merchant(1);let ua=ua_at(&ufvk,0);let bytes=build_tx(&[Out::Ironwood(*ua.orchard().unwrap(),100000,NONCE)]);let txid=audit_txid(&bytes);let ch=challenge(&ua.encode(&NETWORK),100000);let(a,s,_guard,path)=audit_replay_app(&ufvk,bytes).await;
    let r=audit_request(a.clone(),false,audit_headers(),audit_wire(&ch,&txid)).await;assert_eq!(r.status(),200);let v=audit_json(r).await;assert_eq!(v["isValid"],true);assert_eq!(v["status"],"confirmed");assert_eq!(v["payer"],"");assert!(v.get("invalidReason").is_none());assert!(v.get("receipt").is_none());assert_eq!(audit_binding_count(&path),0);audit_mark(221);
    for i in 0..2{let r=audit_request(a.clone(),true,audit_headers(),audit_wire(&ch,&txid)).await;assert_eq!(r.status(),200);let v=audit_json(r).await;assert_eq!(v["success"],true);assert_eq!(v["transaction"],txid);assert_eq!(v["payer"],"");assert_eq!(v["network"],NETWORK_TESTNET);assert_eq!(v["status"],"confirmed");let receipt=v["receipt"].as_str().unwrap();let c=receipt::verify(&a.signing_key.verifying_key(),receipt).unwrap();assert_eq!(c.txid,txid);assert_eq!(c.memo_nonce,ch.memo_nonce);assert_eq!(c.pay_to,ch.pay_to);assert_eq!(c.amount_zat,100000);assert_eq!(c.confirmations,1);assert_eq!(c.mined_at,Some(1792560000));assert_eq!(serde_json::to_value(c).unwrap(),v["claims"]);assert_eq!(audit_binding_count(&path),1);audit_mark(if i==0{222}else{223});}
    let r=audit_request(a.clone(),false,audit_headers(),serde_json::to_vec(&VerifyRequest{challenge:ch.clone(),txid:txid.clone()}).unwrap()).await;assert_eq!(r.status(),200);let v=audit_json(r).await;assert_eq!(v["status"],"confirmed");assert_eq!(receipt::verify(&a.signing_key.verifying_key(),v["receipt"].as_str().unwrap()).unwrap().txid,txid);assert_eq!(audit_binding_count(&path),1);audit_mark(224);
    // Durable binding survives a reopen, and stops another txid on both routes before RPC.
    for settle in [false,true]{let mut other=app();Arc::get_mut(&mut other).unwrap().issued=IssuedStore::open(&path).unwrap();let r=audit_request(other.clone(),settle,audit_headers(),audit_wire(&ch,&"cd".repeat(32))).await;assert_eq!(r.status(),422);let v=audit_json(r).await;assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],"nonce_already_used");assert_eq!(other.issued.txid_for(&ch.memo_nonce,&ch.pay_to,100000).unwrap(),Some(txid.clone()));}audit_mark(229);
    s.lock().unwrap().mode="tip-failure".into();*a.health.lock().await=None;assert_eq!(health(State(a.clone())).await.status(),503);let calls=a.health_probes.load(std::sync::atomic::Ordering::Relaxed);{let mut cache=a.health.lock().await;cache.as_mut().unwrap().0-=HEALTH_CACHE;}
    assert_eq!(health(State(a.clone())).await.status(),503);assert_eq!(a.health_probes.load(std::sync::atomic::Ordering::Relaxed),calls+1);s.lock().unwrap().mode=String::new();{let mut cache=a.health.lock().await;cache.as_mut().unwrap().0-=HEALTH_CACHE;}let r=health(State(a.clone())).await;assert_eq!(r.status(),200);assert_eq!(audit_json(r).await["ok"],true);assert_eq!(a.health_probes.load(std::sync::atomic::Ordering::Relaxed),calls+2);audit_mark(243);
    // A real SQLite trigger failure occurs only after real decryption/expiry/confirmation checks.
    let conn=rusqlite::Connection::open(&path).unwrap();conn.execute_batch("DELETE FROM bindings; CREATE TRIGGER deny_binding BEFORE INSERT ON bindings BEGIN SELECT RAISE(FAIL,'audit binding denied'); END;").unwrap();let r=audit_request(a.clone(),true,audit_headers(),audit_wire(&ch,&txid)).await;assert_eq!(r.status(),503);let v=audit_json(r).await;assert_eq!(v["success"],false);assert_eq!(v["status"],"error");assert_eq!(v["errorReason"],"facilitator_error");assert!(v.get("receipt").is_none());assert_eq!(audit_binding_count(&path),0);audit_mark(467);
    conn.execute_batch("DROP TRIGGER deny_binding; DROP TABLE bindings;").unwrap();for settle in [false,true]{let r=audit_request(a.clone(),settle,audit_headers(),audit_wire(&ch,&txid)).await;assert_eq!(r.status(),503);let v=audit_json(r).await;assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],"facilitator_error");assert!(v["error"].as_str().unwrap().starts_with("facilitator database error:"));assert!(v.get("receipt").is_none());audit_mark(if settle{217}else{188});}drop(conn);drop(a);audit_cleanup_db(&path);
    let store=IssuedStore::open(std::path::Path::new(":memory:")).unwrap();store.bind("n","utest1a",100000,"first",1).unwrap();store.bind("n","utest1b",100000,"second",2).unwrap();assert_eq!(store.txid_for("n","utest1a",100000).unwrap().as_deref(),Some("first"));assert_eq!(store.txid_for("n","utest1b",100000).unwrap().as_deref(),Some("second"));audit_mark(228);
}
#[test]
fn audit_non_text_memo_is_never_credited(){use audit_tx::*;let ufvk=merchant(1);let ua=ua_at(&ufvk,0);let memo=zcash_protocol::memo::MemoBytes::empty();assert_eq!(verify::memo_text(memo.as_array()),None);let data=build_tx(&[Out::NonText(*ua.orchard().unwrap(),100000)]);let h=u32::from(target_height());let tx=verify::parse_tx(&NETWORK,&data,Some(target_height()),target_height()).unwrap();let outputs=verify::incoming_outputs(&NETWORK,uivk_key(&ufvk).uivk(),&tx,target_height());assert!(outputs.iter().all(|o|verify::memo_text(&o.memo).is_none()));assert_eq!(check(&data,&ufvk,&challenge(&ua.encode(&NETWORK),100000),Some(h),h),Err(verify::VerifyError::NoMatchingOutput));audit_mark(235);}
struct AuditChild(Option<std::process::Child>);impl Drop for AuditChild{fn drop(&mut self){if let Some(mut c)=self.0.take(){let pid=c.id();let _=c.kill();let _=c.wait();println!("facilitator PID {pid} killed and reaped");}}}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn audit_facilitator_ufvk_compatibility_and_mainnet_refusal(){
    let ufvk=audit_tx::merchant(1);let h=u64::from(u32::from(audit_tx::target_height()));let state=Arc::new(std::sync::Mutex::new(Replay{data:vec![],height:h,tip:h,time:1792560000,mode:String::new(),chain:"test".into(),calls:0}));let(url,_guard)=audit_grpc_listener(state.clone()).await;
    let binary=PathBuf::from(std::env::var("KAGE_BIN_DIR").unwrap()).join("x402z-facilitator");
    for mainnet in [false,true]{if mainnet{state.lock().unwrap().chain="main".into();}let port=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=port.local_addr().unwrap();assert!(addr.port()>4142);drop(port);let path=audit_db_path();let mut c=std::process::Command::new(&binary);c.env_clear().env("PATH",std::env::var("PATH").unwrap()).env("X402Z_MERCHANT_UFVK",ufvk.encode(&NETWORK).unwrap()).env("X402Z_RECEIPT_SIGNING_KEY",hex::encode([7;32])).env("X402Z_FACILITATOR_TOKEN",TOKEN).env("X402Z_FACILITATOR_DB",&path).env("X402Z_FACILITATOR_BIND",addr.to_string()).env("ZCASH_LWD_URL",&url).current_dir(std::env::temp_dir()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());let mut child=AuditChild(Some(c.spawn().unwrap()));
        if mainnet{for _ in 0..200{if child.0.as_mut().unwrap().try_wait().unwrap().is_some(){break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}let mut c=child.0.take().unwrap();if c.try_wait().unwrap().is_none(){let _=c.kill();panic!("mainnet refusal did not exit");}let o=c.wait_with_output().unwrap();assert_eq!(o.status.code(),Some(1));assert!(String::from_utf8(o.stderr).unwrap().contains("expected testnet ('test')"));audit_mark(257);}else{let http=reqwest::Client::new();let mut value=None;for _ in 0..200{if let Ok(r)=http.get(format!("http://{addr}/health")).send().await{assert_eq!(r.status(),200);value=Some(r.json::<serde_json::Value>().await.unwrap());break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}let v=value.expect("facilitator readiness");assert_eq!(v["viewingKey"],"ufvk (incoming part only; prefer X402Z_MERCHANT_UIVK)");let mut c=child.0.take().unwrap();let pid=c.id();c.kill().unwrap();let o=c.wait_with_output().unwrap();assert!(String::from_utf8(o.stderr).unwrap().contains("The facilitator only needs incoming payments"));println!("facilitator PID {pid} killed and reaped");let key=MerchantKey::from_ufvk(&ufvk);assert!(key.from_full_key);assert_eq!(key.uivk().encode(&NETWORK).unwrap(),ufvk.to_unified_incoming_viewing_key().encode(&NETWORK).unwrap());audit_mark(251);}audit_cleanup_db(&path);
    }
}
