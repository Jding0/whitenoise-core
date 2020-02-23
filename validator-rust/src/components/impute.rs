use std::collections::HashMap;
use crate::utilities::constraint as constraint_utils;
use crate::utilities::constraint::{Constraint, NodeConstraints};

use crate::{base, components};
use crate::proto;
use crate::hashmap;
use crate::components::{Component, Expandable};
use ndarray::Array;

impl Component for proto::Impute {
    // modify min, max, n, categories, is_public, non-null, etc. based on the arguments and component
    fn propagate_constraint(
        &self,
        constraints: &constraint_utils::NodeConstraints,
    ) -> Result<Constraint, String> {
        let mut data_constraint = constraints.get("data").unwrap().clone();
        data_constraint.nullity = false;

        Ok(data_constraint)
    }

    fn is_valid(
        &self,
        constraints: &constraint_utils::NodeConstraints,
    ) -> bool {
        // check these properties are Some
        if constraint_utils::get_min_f64(constraints, "data").is_err()
            || constraint_utils::get_min_f64(constraints, "data").is_err()
            || constraint_utils::get_num_records(constraints, "data").is_err() {
            return false;
        }

        // all checks have passed
        true
    }
}

impl Expandable for proto::Impute {
    fn expand_graph(
        &self,
        privacy_definition: &proto::PrivacyDefinition,
        component: &proto::Component,
        constraints: &constraint_utils::NodeConstraints,
        component_id: u32,
        maximum_id: u32,
    ) -> Result<(u32, HashMap<u32, proto::Component>), String> {
        Ok((maximum_id, HashMap::new()))
    }
}
