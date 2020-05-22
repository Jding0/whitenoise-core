use crate::errors::*;


use std::collections::HashMap;

use crate::{proto, base, Warnable};

use crate::components::Component;
use crate::base::{Value, Jagged, ValueProperties, IndexmapProperties, ArrayProperties};
use crate::utilities::prepend;
use indexmap::map::IndexMap;


impl Component for proto::Partition {
    fn propagate_property(
        &self,
        _privacy_definition: &Option<proto::PrivacyDefinition>,
        public_arguments: &HashMap<String, Value>,
        properties: &base::NodeProperties,
        node_id: u32,
    ) -> Result<Warnable<ValueProperties>> {
        let mut data_property = properties.get("data")
            .ok_or("data: missing")?.array()
            .map_err(prepend("data:"))?.clone();

        Ok(ValueProperties::Indexmap(match properties.get("by") {
            Some(by_property) => {
                let by_property = by_property.array()
                    .map_err(prepend("by:"))?.clone();
                let by_num_columns = by_property.num_columns
                    .ok_or_else(|| Error::from("number of columns must be known on by"))?;
                if by_num_columns != 1 {
                    return Err("Partition's by argument must contain a single column".into());
                }
                let categories = by_property.categories()
                    .map_err(prepend("by:"))?;
                data_property.num_records = None;

                IndexmapProperties {
                    num_records: data_property.num_records,
                    disjoint: true,
                    properties: match categories {
                        Jagged::Bool(categories) => broadcast_partitions(&categories, &data_property, node_id)?.into(),
                        Jagged::Str(categories) => broadcast_partitions(&categories, &data_property, node_id)?.into(),
                        Jagged::I64(categories) => broadcast_partitions(&categories, &data_property, node_id)?.into(),
                        _ => return Err("partitioning based on floats is not supported".into())
                    },
                    dataset_id: Some(node_id as i64),
                    variant: proto::indexmap_properties::Variant::Partition,
                }
            }
            None => {
                let num_partitions = public_arguments.get("num_partitions")
                    .ok_or("num_partitions or by must be passed to Partition")?.array()?.first_i64()?;

                let lengths = match data_property.num_records {
                    Some(num_records) => even_split_lengths(num_records, num_partitions)
                        .into_iter().map(Some).collect(),
                    None => (0..num_partitions)
                        .map(|_| None)
                        .collect::<Vec<Option<i64>>>()
                };

                IndexmapProperties {
                    num_records: data_property.num_records,
                    disjoint: false,
                    properties: lengths.iter().enumerate().map(|(index, partition_num_records)| {
                        let mut partition_property = data_property.clone();
                        partition_property.num_records = *partition_num_records;
                        partition_property.group_id.push(base::GroupId {
                            partition_id: node_id,
                            index: None
                        });
                        (index as i64, ValueProperties::Array(partition_property))
                    }).collect::<IndexMap<i64, ValueProperties>>().into(),
                    dataset_id: Some(node_id as i64),
                    variant: proto::indexmap_properties::Variant::Partition,
                }
            }
        }).into())
    }
}

pub fn even_split_lengths(num_records: i64, num_partitions: i64) -> Vec<i64> {
    (0..num_partitions)
        .map(|index| num_records / num_partitions + (if index >= (num_records % num_partitions) { 0 } else { 1 }))
        .collect()
}

pub fn broadcast_partitions<T: Clone + Eq + std::hash::Hash + Ord>(
    categories: &[Vec<T>], properties: &ArrayProperties, node_id: u32
) -> Result<IndexMap<T, ValueProperties>> {
    if categories.len() != 1 {
        return Err("categories: must be defined for one column".into());
    }
    let mut properties = properties.clone();
    properties.group_id.push(base::GroupId {
        partition_id: node_id,
        index: None
    });
    let partitions = categories[0].clone();
    Ok(partitions.into_iter()
        .map(|v| (v, ValueProperties::Array(properties.clone())))
        .collect())
}


#[cfg(test)]
mod test_partition {
    use crate::components::partition::even_split_lengths;

    fn vec_eq(left: &Vec<i64>, right: &Vec<i64>) -> bool {
        (left.len() == right.len()) && left.iter().zip(right)
            .all(|(a, b)| a == b)
    }

    #[test]
    fn test_units() {
        assert!(vec_eq(
            &even_split_lengths(4, 3),
            &vec![2, 1, 1]));
        assert!(vec_eq(
            &even_split_lengths(5, 3),
            &vec![2, 2, 1]));
        assert!(vec_eq(
            &even_split_lengths(3, 3),
            &vec![1, 1, 1]));
        assert!(vec_eq(
            &even_split_lengths(2, 3),
            &vec![1, 1, 0]));
        assert!(vec_eq(
            &even_split_lengths(2, 0),
            &vec![]));
    }
}