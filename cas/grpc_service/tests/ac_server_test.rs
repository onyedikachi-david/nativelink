// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::io::Cursor;
use std::pin::Pin;

use tonic::{Code, Request, Response, Status};

use prost::Message;
use proto::build::bazel::remote::execution::v2::{action_cache_server::ActionCache, ActionResult, Digest};

use ac_server::AcServer;
use common::DigestInfo;
use store::{create_store, Store, StoreConfig, StoreType};

const INSTANCE_NAME: &str = "foo";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";

async fn insert_into_store<T: Message>(
    store: Pin<&dyn Store>,
    hash: &str,
    action_result: &T,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut store_data = Vec::new();
    action_result.encode(&mut store_data)?;
    let digest = DigestInfo::try_new(&hash, store_data.len() as i64)?;
    store.update(digest.clone(), Box::new(Cursor::new(store_data))).await?;
    Ok(digest.size_bytes as i64)
}

#[cfg(test)]
mod get_action_results {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use proto::build::bazel::remote::execution::v2::GetActionResultRequest;

    async fn get_action_result(ac_server: &AcServer, hash: &str, size: i64) -> Result<Response<ActionResult>, Status> {
        ac_server
            .get_action_result(Request::new(GetActionResultRequest {
                instance_name: INSTANCE_NAME.to_string(),
                action_digest: Some(Digest {
                    hash: hash.to_string(),
                    size_bytes: size,
                }),
                inline_stdout: false,
                inline_stderr: false,
                inline_output_files: vec![],
            }))
            .await
    }

    #[tokio::test]
    async fn empty_store() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store_owned = create_store(&StoreConfig {
            store_type: StoreType::Memory,
            verify_size: false,
        });
        let ac_server = AcServer::new(
            ac_store_owned.clone(),
            create_store(&StoreConfig {
                store_type: StoreType::Memory,
                verify_size: true,
            }),
        );

        let raw_response = get_action_result(&ac_server, HASH1, 0).await;

        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(
            err.message(),
            "Hash 0123456789abcdef000000000000000000000000000000000123456789abcdef not found"
        );
        Ok(())
    }

    #[tokio::test]
    async fn has_single_item() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store_owned = create_store(&StoreConfig {
            store_type: StoreType::Memory,
            verify_size: false,
        });
        let ac_server = AcServer::new(
            ac_store_owned.clone(),
            create_store(&StoreConfig {
                store_type: StoreType::Memory,
                verify_size: true,
            }),
        );

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let ac_store = Pin::new(ac_store_owned.as_ref());
        let digest_size = insert_into_store(ac_store, &HASH1, &action_result).await?;
        let raw_response = get_action_result(&ac_server, HASH1, digest_size).await;

        assert!(raw_response.is_ok(), "Expected value, got error {:?}", raw_response);
        assert_eq!(raw_response.unwrap().into_inner(), action_result);
        Ok(())
    }

    #[tokio::test]
    async fn single_item_wrong_digest_size() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store_owned = create_store(&StoreConfig {
            store_type: StoreType::Memory,
            verify_size: false,
        });
        let ac_server = AcServer::new(
            ac_store_owned.clone(),
            create_store(&StoreConfig {
                store_type: StoreType::Memory,
                verify_size: true,
            }),
        );

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let ac_store = Pin::new(ac_store_owned.as_ref());
        let digest_size = insert_into_store(ac_store, &HASH1, &action_result).await?;
        assert!(digest_size > 1, "Digest was too small");
        let raw_response = get_action_result(&ac_server, HASH1, digest_size - 1).await;

        let err = raw_response.unwrap_err();
        assert_eq!(err.code(), Code::NotFound);
        assert_eq!(err.message(), "Found item, but size does not match");
        Ok(())
    }
}

#[cfg(test)]
mod update_action_result {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use proto::build::bazel::remote::execution::v2::UpdateActionResultRequest;

    fn get_encoded_proto_size<T: Message>(proto: &T) -> Result<usize, Box<dyn std::error::Error>> {
        let mut store_data = Vec::new();
        proto.encode(&mut store_data)?;
        Ok(store_data.len())
    }

    async fn update_action_result(
        ac_server: &AcServer,
        digest: Digest,
        action_result: ActionResult,
    ) -> Result<Response<ActionResult>, Status> {
        ac_server
            .update_action_result(Request::new(UpdateActionResultRequest {
                instance_name: INSTANCE_NAME.to_string(),
                action_digest: Some(digest),
                action_result: Some(action_result),
                results_cache_policy: None,
            }))
            .await
    }

    #[tokio::test]
    async fn one_item_update_test() -> Result<(), Box<dyn std::error::Error>> {
        let ac_store_owned = create_store(&StoreConfig {
            store_type: StoreType::Memory,
            verify_size: false,
        });
        let ac_server = AcServer::new(
            ac_store_owned.clone(),
            create_store(&StoreConfig {
                store_type: StoreType::Memory,
                verify_size: true,
            }),
        );

        let mut action_result = ActionResult::default();
        action_result.exit_code = 45;

        let size_bytes = get_encoded_proto_size(&action_result)? as i64;

        let raw_response = update_action_result(
            &ac_server,
            Digest {
                hash: HASH1.to_string(),
                size_bytes: size_bytes,
            },
            action_result.clone(),
        )
        .await;

        assert!(raw_response.is_ok(), "Expected success, got error {:?}", raw_response);
        assert_eq!(raw_response.unwrap().into_inner(), action_result);

        let mut raw_data = Vec::new();
        let digest = DigestInfo::try_new(&HASH1, size_bytes)?;
        let ac_store = Pin::new(ac_store_owned.as_ref());
        ac_store.get(digest, &mut Cursor::new(&mut raw_data)).await?;

        let decoded_action_result = ActionResult::decode(Cursor::new(&raw_data))?;
        assert_eq!(decoded_action_result, action_result);
        Ok(())
    }
}
