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

use connected_client::ConnectedClient;
use created_swarm::make_swarms;

use eyre::WrapErr;
use maplit::hashmap;
use serde_json::json;

#[tokio::test]
async fn echo_particle() {
    let swarms = make_swarms(1).await;
    let mut client = ConnectedClient::connect_to(swarms[0].multiaddr.clone())
        .await
        .wrap_err("connect client")
        .unwrap();

    let data = hashmap! {
        "name" => json!("folex"),
        "client" => json!(client.peer_id.to_string()),
        "relay" => json!(client.node.to_string()),
    };
    let response = client
        .execute_particle(
            r#"
        (seq
            (call relay ("op" "noop") [])
            (call client ("return" "") [name])
        )"#,
            data.clone(),
        )
        .await
        .unwrap();
    assert_eq!(data["name"], response[0]);
}
