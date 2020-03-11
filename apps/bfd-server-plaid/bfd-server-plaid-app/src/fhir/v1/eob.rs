use crate::ccw_codebook;
use crate::db::PgPool;
use crate::error;
use crate::fhir::util;
use crate::fhir::v1::code_systems;
use crate::fhir::v1::structures::bundle::*;
use crate::fhir::v1::structures::explanation_of_benefit::*;
use crate::fhir::v1::structures::*;
use crate::fhir::v1::util::*;
use crate::models::structs::PartDEvent;
use actix_web::{web, HttpResponse};
use serde::Deserialize;
use std::convert::TryFrom;

#[derive(Deserialize)]
pub struct EobQueryParams {
    patient: String,
}

/// This Actix handler method provides support for queries like
/// `https://example.com/v1/fhir/ExplanationOfBenefit?patient=Patient/1234`. (The route for it is configured
/// in `crate::main()`.)
///
/// It returns an Actix `HttpResponse` containing a JSON-serialized FHIR `Bundle` resource, which itself
/// contains all of the `ExplanationOfBenefit` resources associated with the specified beneficiary/`Patient`.
pub async fn eob_for_bene_id(
    db_pool: web::Data<PgPool>,
    query_params: web::Query<EobQueryParams>,
) -> Result<HttpResponse, error::AppError> {
    let claims = query_claims_partd_by_bene_id(db_pool, query_params).await?;
    let bundle = transform_claims_partd(claims)?;

    Ok(HttpResponse::Ok()
        .content_type("application/fhir+json")
        .json(bundle))
}

/// Parses the specified HTTP query parameters and returns the raw results of the equivalent DB query as a
/// `Future`.
pub async fn query_claims_partd_by_bene_id(
    db_pool: web::Data<PgPool>,
    query_params: web::Query<EobQueryParams>,
) -> Result<Vec<PartDEvent>, error::AppError> {
    let bene_id = util::parse_relative_reference_expected(&query_params.patient, "Patient").ok_or(
        error::AppError::BadRequestError(String::from(
            "Unable to parse the specified 'patient' parameter.",
        )),
    )?;

    // Run Diesel's synchronous query via a Future (that runs the query in a separate thread pool), to
    // ensure it doesn't block the HTTP response threads, then get back to responding once it's done.
    let claims = web::block(move || crate::db::claims_partd_by_bene_id(db_pool, &bene_id)).await?;
    Ok(claims)
}

/// Returns a `Bundle` of `ExplanationOfBenefit`s that represents the specified `PartDEvent`s.
fn transform_claims_partd(claims: Vec<PartDEvent>) -> error::Result<Bundle> {
    let eobs: error::Result<Vec<ExplanationOfBenefit>> =
        claims.iter().map(transform_claim_partd).collect();
    let eobs: Vec<ExplanationOfBenefit> = eobs?;
    let bundle = Bundle {
        id: String::from("TODO"),
        meta: Some(Meta {
            lastUpdated: calculate_max_resource_last_updated(&eobs),
        }),
        r#type: String::from("searchset"),
        // FYI: FHIR has a max of 2,147,483,647, while Rust's u32 has a max of 4,294,967,295.
        total: u32::try_from(claims.len())?,
        link: vec![BundleLink {
            relation: String::from("self"),
            url: String::from("TODO"),
        }],
        entry: eobs
            .into_iter()
            .map(|eob| BundleEntry {
                resource: Resource::ExplanationOfBenefit(eob),
            })
            .collect(),
    };

    Ok(bundle)
}

/// Returns an `ExplanationOfBenefit` that represents the specified `PartDEvent`.
fn transform_claim_partd(claim: &PartDEvent) -> error::Result<ExplanationOfBenefit> {
    let eob = ExplanationOfBenefit::default();
    let mut eob = map_claim_header_common(claim, eob)?;
    eob.identifier.push(create_identifier(
        &ccw_codebook::RX_SRVC_RFRNC_NUM,
        &claim.RX_SRVC_RFRNC_NUM.to_string(),
    ));
    if let Some(ref mut insurance) = eob.insurance {
        if let Some(ref mut coverage) = insurance.coverage {
            coverage.extension = vec![
                create_extension_identifier(
                    &ccw_codebook::PLAN_CNTRCT_REC_ID,
                    create_identifier(&ccw_codebook::PLAN_CNTRCT_REC_ID, &claim.PLAN_CNTRCT_REC_ID),
                ),
                create_extension_identifier(
                    &ccw_codebook::PLAN_PBP_REC_NUM,
                    create_identifier(&ccw_codebook::PLAN_PBP_REC_NUM, &claim.PLAN_PBP_REC_NUM),
                ),
            ];
        }
    }
    if let Some(pd_dt) = claim.PD_DT {
        eob.payment = Some(Payment { date: Some(pd_dt) });
    }

    // Map SRVC_PRVDR_ID and PHRMCY_SRVC_TYPE_CD.
    // FIXME Map SRVC_PRVDR_ID_QLFYR_CD.
    eob.organization = Some(create_reference_to_npi(&claim.SRVC_PRVDR_ID));
    let mut facility = create_reference_to_npi(&claim.SRVC_PRVDR_ID);
    facility.extension = vec![create_extension_coding(
        &ccw_codebook::PHRMCY_SRVC_TYPE_CD,
        create_coding_for_codebook_value(
            &ccw_codebook::PHRMCY_SRVC_TYPE_CD,
            &claim.PHRMCY_SRVC_TYPE_CD,
        ),
    )];
    eob.facility = Some(facility);

    // Map the `ExplanationOfBenefit.information` entries.
    // FIXME Need to map these even when they're null, as the codebook has descriptions for that.
    eob.information.push(create_information_with_code(
        &eob,
        &ccw_codebook::DAW_PROD_SLCTN_CD,
        &claim.DAW_PROD_SLCTN_CD,
    ));
    if let Some(dspnsng_stus_cd) = &claim.DSPNSNG_STUS_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::DSPNSNG_STUS_CD,
            dspnsng_stus_cd,
        ));
    }
    eob.information.push(create_information_with_code(
        &eob,
        &ccw_codebook::DRUG_CVRG_STUS_CD,
        &claim.DRUG_CVRG_STUS_CD,
    ));
    if let Some(adjstmt_dltn_cd) = &claim.ADJSTMT_DLTN_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::ADJSTMT_DLTN_CD,
            adjstmt_dltn_cd,
        ));
    }
    if let Some(nstd_frmt_cd) = &claim.NSTD_FRMT_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::NSTD_FRMT_CD,
            nstd_frmt_cd,
        ));
    }
    if let Some(prcng_excptn_cd) = &claim.PRCNG_EXCPTN_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::PRCNG_EXCPTN_CD,
            prcng_excptn_cd,
        ));
    }
    if let Some(ctstrphc_cvrg_cd) = &claim.CTSTRPHC_CVRG_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::CTSTRPHC_CVRG_CD,
            ctstrphc_cvrg_cd,
        ));
    }
    if let Some(rx_orgn_cd) = &claim.RX_ORGN_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::RX_ORGN_CD,
            rx_orgn_cd,
        ));
    }
    if let Some(brnd_gnrc_cd) = &claim.BRND_GNRC_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::BRND_GNRC_CD,
            brnd_gnrc_cd,
        ));
    }
    // FIXME Why is PHRMCY_SRVC_TYPE_CD mapped twice?
    eob.information.push(create_information_with_code(
        &eob,
        &ccw_codebook::PHRMCY_SRVC_TYPE_CD,
        &claim.PHRMCY_SRVC_TYPE_CD,
    ));
    eob.information.push(create_information_with_code(
        &eob,
        &ccw_codebook::PTNT_RSDNC_CD,
        &claim.PTNT_RSDNC_CD,
    ));
    if let Some(submsn_clr_cd) = &claim.SUBMSN_CLR_CD {
        eob.information.push(create_information_with_code(
            &eob,
            &ccw_codebook::SUBMSN_CLR_CD,
            submsn_clr_cd,
        ));
    }

    // Create the EOB's single Item, its Adjudications, and its single Detail.
    let mut item = explanation_of_benefit::Item::default();
    item.sequence = 1;
    let mut adjudications = vec![];
    let mut detail = explanation_of_benefit::Detail::default();

    // Map the EOB.item.detail.type field from CMPND_CD.
    let compound_code = match claim.CMPND_CD {
        0 => Ok(None),
        1 => Ok(Some(
            &code_systems::explanation_of_benefit::act_invoice_group::RXDINV,
        )),
        2 => Ok(Some(
            &code_systems::explanation_of_benefit::act_invoice_group::RXCINV,
        )),
        _ => Err(error::AppError::InvalidSourceDataError(format!(
            "Unsupported 'CMPND_CD' value."
        ))),
    }?;
    if let Some(compound_code) = compound_code {
        detail.r#type = Some(create_concept_for_value_set_code(compound_code));
    };

    // Map the prescription fill date.
    item.serviced = Some(Serviced::ServicedDate(claim.SRVC_DT));

    /*
     * Create an adjudication for either CVRD_D_PLAN_PD_AMT or NCVRD_PLAN_PD_AMT, depending on the
     * value of DRUG_CVRG_STUS_CD. Stick DRUG_CVRG_STUS_CD into the adjudication.reason field.
     */
    // FIXME should always map both CVRD_D_PLAN_PD_AMT and NCVRD_PLAN_PD_AMT
    let (category, amount) = match claim.DRUG_CVRG_STUS_CD.as_str() {
        "C" => (
            create_adjudication_category_concept(&ccw_codebook::CVRD_D_PLAN_PD_AMT),
            create_money_from_big_decimal(&claim.CVRD_D_PLAN_PD_AMT),
        ),
        _ => (
            create_adjudication_category_concept(&ccw_codebook::NCVRD_PLAN_PD_AMT),
            create_money_from_big_decimal(&claim.NCVRD_PLAN_PD_AMT),
        ),
    };
    let adjudication_drug_payment = Adjudication {
        category: Some(category),
        reason: Some(create_concept_for_codebook_value(
            &ccw_codebook::DRUG_CVRG_STUS_CD,
            &claim.DRUG_CVRG_STUS_CD,
        )),
        amount: Some(amount),
    };
    adjudications.push(adjudication_drug_payment);

    // Map the miscellaneous adjudication amounts.
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::GDC_BLW_OOPT_AMT,
        &claim.GDC_BLW_OOPT_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::GDC_ABV_OOPT_AMT,
        &claim.GDC_ABV_OOPT_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::PTNT_PAY_AMT,
        &claim.PTNT_PAY_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::OTHR_TROOP_AMT,
        &claim.OTHR_TROOP_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::LICS_AMT,
        &claim.LICS_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::PLRO_AMT,
        &claim.PLRO_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::TOT_RX_CST_AMT,
        &claim.TOT_RX_CST_AMT,
    ));
    adjudications.push(create_adjudication_amount(
        &ccw_codebook::RPTD_GAP_DSCNT_NUM,
        &claim.RPTD_GAP_DSCNT_NUM,
    ));

    // Map PRSCRBR_ID_QLFYR_CD.
    match claim.PRSCRBR_ID_QLFYR_CD.as_ref() {
        "01" => {
            // FIXME why don't we map this?
        }
        _ => {
            return Err(error::AppError::InvalidSourceDataError(
                "Invalid PRSCRBR_ID_QLFYR_CD value.".to_string(),
            ));
        }
    }

    // Map PRSCRBR_ID.
    match claim.PRSCRBR_ID.as_ref() {
        "" => {}
        _ => {
            eob = map_care_team_npi(
                eob,
                Some(&mut item),
                &claim.PRSCRBR_ID,
                &code_systems::explanation_of_benefit::care_team_role::PRIMARY,
            )?;
        }
    }

    // Map PROD_SRVC_ID.
    item.service = Some(create_concept_for_ndc(&claim.PROD_SRVC_ID));

    // Map QTY_DSPNSD_NUM, FILL_NUM, and DAYS_SUPLY_NUM.
    item.quantity = Some(create_quantity_from_big_decimal(&claim.QTY_DSPNSD_NUM));
    let fill_num = create_extension_quantity(
        &ccw_codebook::FILL_NUM,
        create_quantity_from_big_decimal(&claim.FILL_NUM),
    );
    let days_supply_num = create_extension_quantity(
        &ccw_codebook::DAYS_SUPLY_NUM,
        create_quantity_from_big_decimal(&claim.DAYS_SUPLY_NUM),
    );
    if let Some(ref mut quantity) = item.quantity {
        quantity.extension = vec![fill_num, days_supply_num];
    };

    // Attach the EOB's single Item, Adjudications, and Detail.
    item.adjudication = adjudications;
    item.detail = vec![detail];
    eob.item = vec![item];

    // TODO flesh out the rest of this

    Ok(eob)
}