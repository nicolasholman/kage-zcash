// Included in facilitator main's test module; real handlers and real SQLite, no chain stubs.
async fn audit_json(r:Response)->serde_json::Value{serde_json::from_slice(&axum::body::to_bytes(r.into_body(),1<<20).await.unwrap()).unwrap()}
fn audit_mark(n:u32){println!("AUDIT B-{n:03} PASS");}
fn audit_headers()->HeaderMap{let mut h=HeaderMap::new();h.insert("authorization",format!("Bearer {TOKEN}").parse().unwrap());h}
async fn audit_request(a:Arc<App>,settle:bool,h:HeaderMap,b:Vec<u8>)->Response{if settle{settle_handler(State(a),h,b.into()).await}else{verify_handler(State(a),h,b.into()).await}}
#[tokio::test]
async fn audit_auth_and_payload_rejections_without_chain() {
    // An actual listening loopback socket counts connection attempts, and never supplies chain data.
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();assert!(addr.port()>4142);
    let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let c=count.clone();let spy=tokio::spawn(async move{while let Ok((stream,_))=listener.accept().await{c.fetch_add(1,std::sync::atomic::Ordering::SeqCst);drop(stream);}});
    let mut a=app();{let a=Arc::get_mut(&mut a).unwrap();a.lwd_url=format!("http://{addr}");a.lwd=zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient::new(tonic::transport::Endpoint::from_shared(a.lwd_url.clone()).unwrap().connect_lazy());}
    let reqs=PaymentRequirements::from_challenge(&ch(),"http://h/r","d","application/json",900);
    let valid=body(PaymentPayload::zcash(&"ab".repeat(32),"x402z-abc"),reqs);
    for settle in [false,true]{let base=if settle{192}else{163};for (i,auth) in [None,Some("Bearer wrong"),Some("Basic wrong"),Some(TOKEN)].into_iter().enumerate(){let mut h=HeaderMap::new();if let Some(auth)=auth{h.insert("authorization",auth.parse().unwrap());}let r=audit_request(a.clone(),settle,h,valid.clone()).await;assert_eq!(r.status(),401);assert_eq!(r.headers()["www-authenticate"],"Bearer");assert_eq!(audit_json(r).await["error"],"facilitator requires Authorization: Bearer <X402Z_FACILITATOR_TOKEN>");assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);assert_eq!(a.issued.txid_for(&ch().memo_nonce,&ch().pay_to,100000).unwrap(),None);audit_mark(base+i as u32);}
        let r=audit_request(a.clone(),settle,audit_headers(),b"{}".to_vec()).await;assert_eq!(r.status(),400);assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);audit_mark(base+4);
        for (i,why,error) in [(0,"invalid_x402_version","x402Version 2 not supported"),(1,"invalid_network","paymentPayload:"),(2,"invalid_scheme","paymentPayload:"),(3,"invalid_payload","paymentPayload.memoNonce does not match paymentRequirements.extra.memoNonce"),(4,"invalid_payload","txid must be 64 hex chars"),(5,"invalid_payload","invalid challenge:")] {
            let mut b:serde_json::Value=serde_json::from_slice(&valid).unwrap();match i{0=>b["x402Version"]=2.into(),1=>b["paymentPayload"]["network"]="zcash-mainnet".into(),2=>b["paymentPayload"]["scheme"]="exact".into(),3=>b["paymentPayload"]["payload"]["memoNonce"]="other".into(),4=>b["paymentPayload"]["payload"]["txid"]="zz".into(),_=>b["paymentRequirements"]["payTo"]="utest1short".into()};
            let r=audit_request(a.clone(),settle,audit_headers(),serde_json::to_vec(&b).unwrap()).await;assert_eq!(r.status(),400);let v=audit_json(r).await;if settle{assert_eq!(v["success"],false);assert_eq!(v["errorReason"],why);assert_eq!(v["status"],"rejected");}else{assert_eq!(v["isValid"],false);assert_eq!(v["invalidReason"],why);assert_eq!(v["payer"],"");}assert!(v["error"].as_str().unwrap().starts_with(error),"unexpected error for case {i}");assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);assert_eq!(a.issued.txid_for(&ch().memo_nonce,&ch().pay_to,100000).unwrap(),None);audit_mark(if settle{197+i}else{168+i});
        }
    }
    for b in [b"{".to_vec(),b"{}".to_vec()]{let r=audit_request(a.clone(),false,audit_headers(),b).await;assert_eq!(r.status(),400);let v=audit_json(r).await;assert_eq!(v["status"],"rejected");assert!(v["error"].as_str().unwrap().starts_with("bad request body:"));}assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);audit_mark(225);
    let r=audit_request(a.clone(),true,audit_headers(),b"{".to_vec()).await;assert_eq!(r.status(),400);let v=audit_json(r).await;assert_eq!(v["success"],false);assert_eq!(v["status"],"rejected");assert_eq!(v["errorReason"],"invalid_payload");assert!(v["error"].as_str().unwrap().starts_with("bad request body:"));audit_mark(226);
    spy.abort();
}
#[tokio::test]
async fn audit_real_binding_pin_and_outage() {
    for settle in [false,true]{
        let mut a=app();Arc::get_mut(&mut a).unwrap().merchant_ua=Some("utest1differentmerchant00000000000".into());let reqs=PaymentRequirements::from_challenge(&ch(),"http://h/r","d","application/json",900);let b=body(PaymentPayload::zcash(&"ab".repeat(32),"x402z-abc"),reqs);
        let r=audit_request(a.clone(),settle,audit_headers(),b.clone()).await;assert_eq!(r.status(),422);let v=audit_json(r).await;assert_eq!(v["error"],"payTo is not this merchant's address");assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],"invalid_payment");assert_eq!(v[if settle{"success"}else{"isValid"}],false);if !settle{assert_eq!(v["payer"],"");}assert_eq!(a.issued.txid_for(&ch().memo_nonce,&ch().pay_to,100000).unwrap(),None);audit_mark(if settle{206}else{177});
        Arc::get_mut(&mut a).unwrap().merchant_ua=None;
        a.issued.bind(&ch().memo_nonce,&ch().pay_to,100000,&"cd".repeat(32),1).unwrap();
        let r=audit_request(a.clone(),settle,audit_headers(),b).await;assert_eq!(r.status(),422);let v=audit_json(r).await;assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],"nonce_already_used");assert_eq!(v[if settle{"success"}else{"isValid"}],false);assert!(v["error"].as_str().unwrap().starts_with("memo nonce already receipted for tx"));assert_eq!(a.issued.txid_for(&ch().memo_nonce,&ch().pay_to,100000).unwrap(),Some("cd".repeat(32)));audit_mark(if settle{211}else{182});
        let a=app();let reqs=PaymentRequirements::from_challenge(&ch(),"http://h/r","d","application/json",900);let b=body(PaymentPayload::zcash(&"ab".repeat(32),"x402z-abc"),reqs);let r=audit_request(a.clone(),settle,audit_headers(),b).await;assert_eq!(r.status(),502);let v=audit_json(r).await;assert_eq!(v[if settle{"errorReason"}else{"invalidReason"}],"facilitator_error");assert_eq!(v[if settle{"success"}else{"isValid"}],false);assert!(v["error"].as_str().unwrap().contains("127.0.0.1:9"));assert!(v["error"].as_str().unwrap().contains("lightwalletd"));assert_eq!(a.issued.txid_for(&ch().memo_nonce,&ch().pay_to,100000).unwrap(),None);audit_mark(if settle{215}else{186});
    }
    let a=app();for _ in 0..3{let r=health(State(a.clone())).await;assert_eq!(r.status(),503);let v=audit_json(r).await;assert_eq!(v["ok"],false);assert_eq!(v["lightwalletd"],a.lwd_url);assert!(v["error"].as_str().is_some_and(|s|!s.is_empty()));}assert_eq!(a.health_probes.load(std::sync::atomic::Ordering::Relaxed),1);audit_mark(240);audit_mark(242);
}

include!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../audit/exec/fac-grpc.rs"));
