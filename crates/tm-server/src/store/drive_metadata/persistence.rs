use chrono::Utc;
use tm_drive::{
    DriveCorrectionRecord, DriveEntry, DriveError, DriveLinkRecord, OrganizerProposal, OrganizerRun,
};
use tokio_postgres::GenericClient;

use super::records::{enum_label, json_value, store_error, to_i64};

pub(super) async fn insert_correction<C: GenericClient + Sync>(
    client: &C,
    correction: &DriveCorrectionRecord,
) -> tm_drive::Result<()> {
    let record = json_value(correction)?;
    client
        .execute(
            "insert into drive_corrections(id,version,from_path,to_path,created_at,record_json)
             values($1,$2,$3,$4,$5,$6)",
            &[
                &correction.id,
                &to_i64("correction version", correction.version)?,
                &correction.from,
                &correction.to,
                &correction.created_at,
                &record,
            ],
        )
        .await
        .map_err(store_error)?;
    Ok(())
}

pub(super) async fn tombstone_and_delete_entry<C: GenericClient + Sync>(
    client: &C,
    entry: &DriveEntry,
) -> tm_drive::Result<()> {
    let record = json_value(entry)?;
    client
        .execute(
            "insert into drive_entry_tombstones(id,version,path,entry_json,deleted_at)
             values($1,$2,$3,$4,$5)
             on conflict(id) do update set version=excluded.version,path=excluded.path,
                entry_json=excluded.entry_json,deleted_at=excluded.deleted_at",
            &[
                &entry.id,
                &to_i64("entry version", entry.version)?,
                &entry.path,
                &record,
                &Utc::now(),
            ],
        )
        .await
        .map_err(store_error)?;
    let deleted = client
        .execute(
            "delete from drive_entries where id=$1 and version=$2",
            &[&entry.id, &to_i64("entry version", entry.version)?],
        )
        .await
        .map_err(store_error)?;
    if deleted == 0 {
        return Err(DriveError::Conflict {
            entity: "entry",
            id: entry.id.to_string(),
            expected: entry.version,
            actual: entry.version,
        });
    }
    Ok(())
}

pub(super) async fn write_entry<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    entry: &DriveEntry,
) -> Result<u64, tokio_postgres::Error> {
    let provenance = serde_json::to_value(&entry.provenance).unwrap();
    let entities = serde_json::to_value(&entry.entities).unwrap();
    let dates = serde_json::to_value(&entry.dates).unwrap();
    let amounts = serde_json::to_value(&entry.amounts).unwrap();
    let record = serde_json::to_value(entry).unwrap();
    let values: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[
        &entry.id,
        &i64::try_from(entry.version).unwrap(),
        &entry.path,
        &entry.uri,
        &entry.blob_uri,
        &entry.content_hash,
        &entry.mime,
        &i64::try_from(entry.size_bytes).unwrap(),
        &entry.title,
        &entry.doc_kind,
        &entry.project,
        &entry.source_uri,
        &provenance,
        &entry.summary,
        &enum_label(&entry.status).unwrap(),
        &entry.created_at,
        &entry.updated_at,
        &entities,
        &dates,
        &amounts,
        &entry.embedding,
        &record,
    ];
    if let Some(expected) = expected {
        let mut update_values = Vec::with_capacity(values.len() + 1);
        update_values.push(&entry.id as &(dyn tokio_postgres::types::ToSql + Sync));
        let expected = i64::try_from(expected).unwrap();
        update_values.push(&expected);
        update_values.extend_from_slice(&values[1..]);
        client
            .execute(
                "update drive_entries set version=$3,path=$4,uri=$5,blob_uri=$6,content_hash=$7,
             mime=$8,size_bytes=$9,title=$10,doc_kind=$11,project=$12,source_uri=$13,
             provenance_json=$14,summary=$15,status=$16,created_at=$17,updated_at=$18,
             entities_json=$19,dates_json=$20,amounts_json=$21,embedding=$22,record_json=$23
             where id=$1 and version=$2",
                &update_values,
            )
            .await
    } else {
        client.execute(
            "insert into drive_entries(id,version,path,uri,blob_uri,content_hash,mime,size_bytes,
             title,doc_kind,project,source_uri,provenance_json,summary,status,created_at,updated_at,
             entities_json,dates_json,amounts_json,embedding,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
            values,
        ).await
    }
}

pub(super) async fn replace_entry_children<C: GenericClient + Sync>(
    client: &C,
    entry: &DriveEntry,
) -> tm_drive::Result<()> {
    client
        .execute(
            "delete from drive_attributes where entry_id=$1",
            &[&entry.id],
        )
        .await
        .map_err(store_error)?;
    for (index, attribute) in entry.attributes.iter().enumerate() {
        let evidence = attribute.evidence.as_ref().map(json_value).transpose()?;
        client
            .execute(
                "insert into drive_attributes(entry_id,idx,key,value,confidence,evidence_json,
             extractor,source_uri,session_id,event_seq,content_hash)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
                &[
                    &entry.id,
                    &i32::try_from(index)
                        .map_err(|_| DriveError::Store("too many drive attributes".to_string()))?,
                    &attribute.key,
                    &attribute.value,
                    &attribute.confidence,
                    &evidence,
                    &attribute.extractor,
                    &attribute.source_uri,
                    &attribute.session_id,
                    &attribute.event_seq,
                    &attribute.content_hash,
                ],
            )
            .await
            .map_err(store_error)?;
    }
    client
        .execute("delete from drive_tags where entry_id=$1", &[&entry.id])
        .await
        .map_err(store_error)?;
    for tag in &entry.tags {
        client
            .execute(
                "insert into drive_tags(entry_id,tag) values($1,$2) on conflict do nothing",
                &[&entry.id, tag],
            )
            .await
            .map_err(store_error)?;
    }
    Ok(())
}

pub(super) async fn write_proposal<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    proposal: &OrganizerProposal,
) -> tm_drive::Result<u64> {
    let proposed_tags = json_value(&proposal.proposed_tags)?;
    let evidence = json_value(&proposal.evidence)?;
    let replay = json_value(&proposal.replay_metadata)?;
    let record = json_value(proposal)?;
    if let Some(expected) = expected {
        client.execute(
            "update drive_proposals set version=$3,action=$4,
             entry_id=case when exists(select 1 from drive_entries where id=$5) then $5 else null end,
             entry_id_snapshot=$5,source_path=$6,proposed_path=$7,proposed_tags=$8,
             proposed_doc_kind=$9,proposed_project=$10,evidence_json=$11,confidence=$12,
             policy_decision=$13,approval_id=$14,status=$15,source_run_id=$16,
             replay_metadata=$17,created_at=$18,updated_at=$19,record_json=$20
             where id=$1 and version=$2",
            &[&proposal.id,&to_i64("expected proposal version", expected)?,
              &to_i64("proposal version", proposal.version)?,&enum_label(&proposal.action)?,
              &proposal.entry_id,&proposal.source_path,&proposal.proposed_path,&proposed_tags,
              &proposal.proposed_doc_kind,&proposal.proposed_project,&evidence,&proposal.confidence,
              &enum_label(&proposal.policy_decision)?,&proposal.approval_id,
              &enum_label(&proposal.status)?,&proposal.source_run_id,&replay,
              &proposal.created_at,&proposal.updated_at,&record],
        ).await.map_err(store_error)
    } else {
        client.execute(
            "insert into drive_proposals(id,version,action,entry_id,entry_id_snapshot,source_path,
             proposed_path,proposed_tags,proposed_doc_kind,proposed_project,evidence_json,confidence,
             policy_decision,approval_id,status,source_run_id,replay_metadata,created_at,updated_at,record_json)
             values($1,$2,$3,$4,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
            &[&proposal.id,&to_i64("proposal version", proposal.version)?,
              &enum_label(&proposal.action)?,&proposal.entry_id,&proposal.source_path,
              &proposal.proposed_path,&proposed_tags,&proposal.proposed_doc_kind,
              &proposal.proposed_project,&evidence,&proposal.confidence,
              &enum_label(&proposal.policy_decision)?,&proposal.approval_id,
              &enum_label(&proposal.status)?,&proposal.source_run_id,&replay,
              &proposal.created_at,&proposal.updated_at,&record],
        ).await.map_err(store_error)
    }
}

pub(super) async fn write_run<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    run: &OrganizerRun,
) -> tm_drive::Result<u64> {
    let proposal_ids = json_value(&run.proposal_ids)?;
    let record = json_value(run)?;
    let attempts = i32::try_from(run.attempts)
        .map_err(|_| DriveError::Store("organizer attempts overflow".to_string()))?;
    if let Some(expected) = expected {
        client
            .execute(
                "update drive_organizer_runs set version=$3,trigger=$4,status=$5,attempts=$6,
             proposal_ids=$7,created_at=$8,available_at=$9,locked_at=$10,completed_at=$11,
             last_error=$12,record_json=$13 where id=$1 and version=$2",
                &[
                    &run.id,
                    &to_i64("expected organizer run version", expected)?,
                    &to_i64("organizer run version", run.version)?,
                    &run.trigger,
                    &enum_label(&run.status)?,
                    &attempts,
                    &proposal_ids,
                    &run.created_at,
                    &run.available_at,
                    &run.locked_at,
                    &run.completed_at,
                    &run.last_error,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    } else {
        client
            .execute(
                "insert into drive_organizer_runs(id,version,trigger,status,attempts,proposal_ids,
             created_at,available_at,locked_at,completed_at,last_error,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
                &[
                    &run.id,
                    &to_i64("organizer run version", run.version)?,
                    &run.trigger,
                    &enum_label(&run.status)?,
                    &attempts,
                    &proposal_ids,
                    &run.created_at,
                    &run.available_at,
                    &run.locked_at,
                    &run.completed_at,
                    &run.last_error,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    }
}

pub(super) async fn write_link<C: GenericClient + Sync>(
    client: &C,
    expected: Option<u64>,
    link: &DriveLinkRecord,
) -> tm_drive::Result<u64> {
    let metadata = json_value(&link.metadata)?;
    let record = json_value(link)?;
    if let Some(expected) = expected {
        client
            .execute(
                "update drive_links set version=$3,canonical_root=$4,mode=$5,linked_uri=$6,
             memory_scope=$7,project=$8,status=$9,metadata_json=$10,created_at=$11,
             updated_at=$12,revoked_at=$13,record_json=$14 where alias=$1 and version=$2",
                &[
                    &link.alias,
                    &to_i64("expected link version", expected)?,
                    &to_i64("link version", link.version)?,
                    &link.canonical_root,
                    &link.mode,
                    &link.linked_uri,
                    &link.memory_scope,
                    &link.project,
                    &enum_label(&link.status)?,
                    &metadata,
                    &link.created_at,
                    &link.updated_at,
                    &link.revoked_at,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    } else {
        client
            .execute(
                "insert into drive_links(alias,version,canonical_root,mode,linked_uri,memory_scope,
             project,status,metadata_json,created_at,updated_at,revoked_at,record_json)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
                &[
                    &link.alias,
                    &to_i64("link version", link.version)?,
                    &link.canonical_root,
                    &link.mode,
                    &link.linked_uri,
                    &link.memory_scope,
                    &link.project,
                    &enum_label(&link.status)?,
                    &metadata,
                    &link.created_at,
                    &link.updated_at,
                    &link.revoked_at,
                    &record,
                ],
            )
            .await
            .map_err(store_error)
    }
}
