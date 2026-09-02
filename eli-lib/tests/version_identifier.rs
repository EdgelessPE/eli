use eli_lib::version_identifier::{
    EdgelessVersion, EdgelessVersionIdentifier, ReleaseChannel, ReleaseStage, UpdateMethod,
};

#[test]
fn public_api_parses_all_supported_identifier_specs() {
    let current: EdgelessVersionIdentifier = "Edgeless_Beta_Ofial_4.1.0_2".parse().unwrap();
    assert_eq!(current.stage, ReleaseStage::Beta);
    assert_eq!(current.channel, Some(ReleaseChannel::Official));
    assert_eq!(
        current.version,
        EdgelessVersion {
            major: 4,
            minor: 1,
            patch: 0
        }
    );
    assert_eq!(current.update_method, Some(UpdateMethod::ComponentsAndWim));

    let alpha = EdgelessVersionIdentifier::parse("Edgeless_Alpha_4.1.2").unwrap();
    assert_eq!(alpha.stage, ReleaseStage::Alpha);
    assert_eq!(alpha.channel, None);
    assert_eq!(alpha.update_method, None);

    let beta = EdgelessVersionIdentifier::parse("Edgeless_Beta_4.1.0").unwrap();
    assert_eq!(beta.stage, ReleaseStage::Beta);
    assert_eq!(beta.version.to_string(), "4.1.0");
}
