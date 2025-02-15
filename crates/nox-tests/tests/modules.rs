/*
 * Copyright 2024 Fluence DAO
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use base64::{engine::general_purpose::STANDARD as base64, Engine};
use connected_client::ConnectedClient;
use created_swarm::{make_swarms, make_swarms_with_cfg};
use maplit::hashmap;
use serde_json::json;
use service_modules::load_module;

#[tokio::test]
async fn test_add_module_mounted_binaries() {
    let swarms = make_swarms_with_cfg(1, move |mut cfg| {
        cfg.allowed_effectors = hashmap! {
            "bafkreiepzclggkt57vu7yrhxylfhaafmuogtqly7wel7ozl5k2ehkd44oe".to_string() => hashmap! {
                "ls".to_string() => "/bin/ls".to_string()
            }
        };
        cfg
    })
    .await;

    let mut client = ConnectedClient::connect_with_keypair(
        swarms[0].multiaddr.clone(),
        Some(swarms[0].management_keypair.clone()),
    )
    .await
    .expect("connect client");
    let module = load_module("tests/effector/artifacts", "effector").expect("load module");

    let config = json!(
    {
        "name": "effector",
        "mem_pages_count": 100,
        "logger_enabled": true,
        "wasi": {
            "envs": json!({}),
            "preopened_files": vec!["/tmp"],
            "mapped_dirs": json!({}),
        },
        "mounted_binaries": json!({"ls": "/bin/ls"})
    });

    let script = r#"
    (xor
       (seq
           (call node ("dist" "add_module") [module_bytes module_config])
           (call client ("return" "") ["ok"])
       )
       (call client ("return" "") [%last_error%.$.message])
    )
   "#;

    let data = hashmap! {
        "client" => json!(client.peer_id.to_string()),
        "node" => json!(client.node.to_string()),
        "module_bytes" => json!(base64.encode(&module)),
        "module_config" => config,
    };

    let response = client.execute_particle(script, data).await.unwrap();
    if let Some(result) = response[0].as_str() {
        assert_eq!("ok", result);
    } else {
        panic!("can't receive response from node");
    }
}

#[tokio::test]
async fn test_add_module_effectors_forbidden() {
    let swarms = make_swarms(1).await;

    let mut client = ConnectedClient::connect_with_keypair(
        swarms[0].multiaddr.clone(),
        Some(swarms[0].management_keypair.clone()),
    )
    .await
    .expect("connect client");
    let module = load_module("tests/effector/artifacts", "effector").expect("load module");

    let config = json!(
    {
        "name": "tetraplets",
        "mem_pages_count": 100,
        "logger_enabled": true,
        "wasi": {
            "envs": json!({}),
            "mapped_dirs": json!({}),
        },
        "mounted_binaries": json!({"cmd": "/usr/bin/behbehbeh"})
    });

    let script = r#"
    (xor
       (seq
           (call node ("dist" "add_module") [module_bytes module_config])
           (call client ("return" "") ["ok"])
       )
       (call client ("return" "") [%last_error%.$.message])
    )
   "#;

    let data = hashmap! {
        "client" => json!(client.peer_id.to_string()),
        "node" => json!(client.node.to_string()),
        "module_bytes" => json!(base64.encode(&module)),
        "module_config" => config,
    };

    let response = client.execute_particle(script, data).await.unwrap();
    if let Some(result) = response[0].as_str() {
        let expected = "Local service error, ret_code is 1, error message is '\"Error: Config error: requested module effector tetraplets with CID bafkreiepzclggkt57vu7yrhxylfhaafmuogtqly7wel7ozl5k2ehkd44oe is forbidden on this host\\nForbiddenEffector { module_name: \\\"tetraplets\\\", forbidden_cid: \\\"bafkreiepzclggkt57vu7yrhxylfhaafmuogtqly7wel7ozl5k2ehkd44oe\\\" }\"'";
        assert_eq!(expected, result);
    } else {
        panic!("can't receive response from node");
    }
}

#[tokio::test]
async fn test_add_module_by_other_forbidden() {
    let swarms = make_swarms(1).await;
    let mut client = ConnectedClient::connect_to(swarms[0].multiaddr.clone())
        .await
        .unwrap();
    let module = load_module("tests/effector/artifacts", "effector").expect("load module");

    let config = json!(
    {
        "name": "tetraplets",
        "mem_pages_count": 100,
        "logger_enabled": true,
        "wasi": {
            "envs": json!({}),
            "mapped_dirs": json!({}),
        },
        "mounted_binaries": json!({"cmd": "/usr/bin/behbehbeh"})
    });

    let script = r#"
    (xor
       (seq
           (call node ("dist" "add_module") [module_bytes module_config])
           (call client ("return" "") ["shouldn't add module"])
       )
       (call client ("return" "") [%last_error%.$.message])
    )
   "#;

    let data = hashmap! {
        "client" => json!(client.peer_id.to_string()),
        "node" => json!(client.node.to_string()),
        "module_bytes" => json!(base64.encode(&module)),
        "module_config" => config,
    };
    let response = client.execute_particle(script, data).await.unwrap();
    assert!(
        response[0]
            .as_str()
            .unwrap()
            .contains("function is only available to the host or worker spells"),
        "got {:?}",
        response[0]
    );
}
