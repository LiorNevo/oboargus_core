use crate::data_dictionary::export_system_schema;
use crate::domain::model_description::model_description;

#[test]
fn every_field_carries_its_declared_aggregates() {
    let schema = export_system_schema().expect("schema");
    let definitions = schema.document().get("$defs").expect("$defs");
    for entity in model_description().entities {
        let definition = definitions.get(entity.name).expect("entity definition");
        for field in entity.fields {
            let property = definition
                .get("properties")
                .and_then(|value| value.get(field.name))
                .expect("field property");
            let aggregates = property
                .get("x-obolargus-aggregates")
                .and_then(|value| value.as_array())
                .expect("aggregates");
            let actual = aggregates
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, field.aggregates, "{}.{}", entity.name, field.name);
        }
    }
}
