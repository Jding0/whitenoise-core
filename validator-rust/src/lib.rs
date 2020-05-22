//! The Whitenoise rust validator contains methods for evaluating and constructing
//! differentially private analyses.
//!
//! The validator defines a set of statically checkable properties that are
//! necessary for a differentially private analysis, and then checks that the submitted analysis
//! satisfies the properties.
//!
//! The validator also takes simple components from the Whitenoise runtime and combines them
//! into more complex mechanisms.
//!
//! - [Top-level documentation](https://opendifferentialprivacy.github.io/whitenoise-core/)

// `error_chain!` can recurse deeply
#![recursion_limit = "1024"]
#[macro_use]
extern crate error_chain;

#[doc(hidden)]
pub mod errors {
    // Create the Error, ErrorKind, ResultExt, and Result types
    error_chain! {}
}

pub struct Warnable<T>(T, Vec<Error>);
impl<T> Warnable<T> {
    pub fn new(value: T) -> Self {
        Warnable(value, Vec::new())
    }
}
impl<T> From<T> for Warnable<T> {
    fn from(value: T) -> Self {
        Warnable::new(value)
    }
}

#[doc(hidden)]
pub use errors::*;
// trait which holds `display_chain`

pub mod base;
pub mod bindings;
pub mod utilities;
pub mod components;
pub mod docs;

// import all trait implementations
use crate::components::*;
use std::collections::{HashMap, HashSet};
use crate::utilities::serial::{serialize_value_properties, serialize_error};
use crate::base::{ReleaseNode, Value};
use std::iter::FromIterator;
use std::ops::{Div, Mul, Add};
use crate::utilities::privacy::compute_graph_privacy_usage;

// for accuracy guarantees
extern crate statrs;

// include protobuf-generated traits
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/whitenoise.rs"));
}

// define the useful macro for building hashmaps globally
#[macro_export]
#[doc(hidden)]
macro_rules! hashmap {
    ($( $key: expr => $val: expr ),*) => {{
         #[allow(unused_mut)]
         let mut map = ::std::collections::HashMap::new();
         $( map.insert($key, $val); )*
         map
    }}
}


/// Validate if an analysis is well-formed.
///
/// Checks that the graph is a DAG.
/// Checks that static properties are met on all components.
///
/// Useful for static validation of an analysis.
/// Since some components require public arguments, mechanisms that depend on other mechanisms cannot be verified until the components they depend on have been validated.
///
/// The system may also be run dynamically- prior to expanding each node, calling the expand_component endpoint will also validate the component being expanded.
/// NOTE: Evaluating the graph dynamically opens up additional potential timing attacks.
pub fn validate_analysis(
    mut analysis: proto::Analysis,
    mut release: base::Release
) -> Result<()> {
    utilities::propagate_properties(&mut analysis, &mut release, None, false)?;
    Ok(())
}


/// Compute overall privacy usage of an analysis.
///
/// The privacy usage is sum of the privacy usages for each node.
/// The Release's actual privacy usage, if defined, takes priority over the maximum allowable privacy usage defined in the Analysis.
pub fn compute_privacy_usage(
    mut analysis: proto::Analysis,
    mut release: base::Release
) -> Result<proto::PrivacyUsage> {

    let properties = utilities::propagate_properties(&mut analysis, &mut release, None, false)?.0;
    // this is mutated within propagate_properties
    let graph = analysis.computation_graph
        .ok_or_else(|| Error::from("computation_graph must be defined"))?.value;
    let privacy_definition = analysis.privacy_definition
        .ok_or_else(|| "privacy_definition must be defined")?;

    let privacy_usage = compute_graph_privacy_usage(
        &graph, &privacy_definition, &properties, &release)?;

    utilities::privacy::privacy_usage_check(&privacy_usage, None, false)?;

    Ok(privacy_usage)
}


/// Generate a json string with a summary/report of the Analysis and Release
pub fn generate_report(
    mut analysis: proto::Analysis,
    mut release: base::Release
) -> Result<String> {

    let graph_properties = utilities::propagate_properties(&mut analysis, &mut release, None, false)?.0;

    let graph = analysis.computation_graph
        .ok_or("computation_graph must be defined")?
        .value;

    // variable names
    let mut nodes_varnames: HashMap<u32, Vec<String>> = HashMap::new();

    utilities::get_traversal(&graph)?.iter().map(|node_id| {
        let component: proto::Component = graph.get(&node_id).unwrap().to_owned();
        let public_arguments = utilities::get_public_arguments(&component, &release)?;

        // variable names for argument nodes
        let mut arguments_vars: HashMap<String, Vec<String>> = HashMap::new();

        // iterate through argument nodes
        for (field_id, field) in &component.arguments {
            // get variable names corresponding to that argument
            if let Some(arg_vars) = nodes_varnames.get(field) {
                arguments_vars.insert(field_id.clone(), arg_vars.clone());
            }
        }

        // get variable names for this node
        let node_vars = component.get_names(
            &public_arguments,
            &arguments_vars,
            release.get(node_id).map(|v| v.value.clone()).as_ref());

        // update names in indexmap
        node_vars.map(|v| nodes_varnames.insert(node_id.clone(), v)).ok();

        Ok(())
    }).collect::<Result<()>>()
        // ignore any error- still generate the report even if node names could not be derived
        .ok();

    let release_schemas = graph.iter()
        .map(|(node_id, component)| {
            let public_arguments = utilities::get_public_arguments(&component, &release)?;
            let input_properties = utilities::get_input_properties(&component, &graph_properties)?;
            let variable_names = nodes_varnames.get(&node_id);
            // ignore nodes without released values
            let node_release = match release.get(node_id) {
                Some(node_release) => node_release.value.clone(),
                None => return Ok(None)
            };
            component.summarize(
                &node_id,
                &component,
                &public_arguments,
                &input_properties,
                &node_release,
                variable_names,
            )
        })
        .collect::<Result<Vec<Option<Vec<utilities::json::JSONRelease>>>>>()?.into_iter()
        .filter_map(|v| v).flat_map(|v| v)
        .collect::<Vec<utilities::json::JSONRelease>>();

    match serde_json::to_string(&release_schemas) {
        Ok(serialized) => Ok(serialized),
        Err(_) => Err("unable to parse report into json".into())
    }
}


/// Estimate the privacy usage necessary to bound accuracy to a given value.
///
/// No context about the analysis is necessary, just the privacy definition and properties of the arguments of the component.
pub fn accuracy_to_privacy_usage(
    component: proto::Component,
    privacy_definition: proto::PrivacyDefinition,
    properties: HashMap<String, proto::ValueProperties>,
    accuracies: proto::Accuracies
) -> Result<proto::PrivacyUsages> {

    let proto_properties = component.arguments.iter()
        .filter_map(|(name, idx)| Some((idx.clone(), properties.get(name)?.clone())))
        .collect::<HashMap<u32, proto::ValueProperties>>();

    let mut analysis = proto::Analysis {
        computation_graph: Some(proto::ComputationGraph {
            value: hashmap![component.arguments.values().max().cloned().unwrap_or(0) + 1 => component.clone()]
        }),
        privacy_definition: Some(privacy_definition.clone()),
    };

    let (properties, _) = utilities::propagate_properties(
        &mut analysis,
        &mut HashMap::new(),
        Some(proto_properties),
        false,
    )?;

    let graph = analysis.computation_graph
        .ok_or("computation_graph must be defined")?
        .value;

    let privacy_usages = graph.iter().map(|(idx, component)| {
        let component_properties = component.arguments.iter()
            .filter_map(|(name, idx)| Some((name.clone(), properties.get(idx)?.clone())))
            .collect::<HashMap<String, base::ValueProperties>>();

        Ok(match component.accuracy_to_privacy_usage(
            &privacy_definition, &component_properties, &accuracies)? {
            Some(accuracies) => Some((idx.clone(), accuracies)),
            None => None
        })
    })
        .collect::<Result<Vec<Option<(u32, Vec<proto::PrivacyUsage>)>>>>()?
        .into_iter().filter_map(|v| v)
        .collect::<HashMap<u32, Vec<proto::PrivacyUsage>>>();

    Ok(proto::PrivacyUsages {
        values: privacy_usages.into_iter().map(|(_, v)| v).collect::<Vec<Vec<proto::PrivacyUsage>>>()
            .first()
            .ok_or_else(|| Error::from("privacy usage is not defined"))?.clone()
    })
}


/// Estimate the accuracy of the release of a component, based on a privacy usage.
///
/// No context about the analysis is necessary, just the properties of the arguments of the component.
pub fn privacy_usage_to_accuracy(
    component: proto::Component,
    privacy_definition: proto::PrivacyDefinition,
    properties: HashMap<String, proto::ValueProperties>,
    alpha: f64
) -> Result<proto::Accuracies> {

    let proto_properties = component.arguments.iter()
        .filter_map(|(name, idx)| Some((idx.clone(), properties.get(name)?.clone())))
        .collect::<HashMap<u32, proto::ValueProperties>>();

    let mut analysis = proto::Analysis {
        computation_graph: Some(proto::ComputationGraph {
            value: hashmap![component.arguments.values().max().cloned().unwrap_or(0) + 1 => component.clone()]
        }),
        privacy_definition: Some(privacy_definition.clone()),
    };

    let (properties, _) = utilities::propagate_properties(
        &mut analysis,
        &mut HashMap::new(),
        Some(proto_properties),
        false,
    )?;

    let graph = analysis.computation_graph
        .ok_or("computation_graph must be defined")?
        .value;

    let accuracies = graph.iter().map(|(idx, component)| {
        let component_properties = component.arguments.iter()
            .filter_map(|(name, idx)| Some((name.clone(), properties.get(idx)?.clone())))
            .collect::<HashMap<String, base::ValueProperties>>();

        Ok(match component.privacy_usage_to_accuracy(
            &privacy_definition, &component_properties, &alpha)? {
            Some(accuracies) => Some((idx.clone(), accuracies)),
            None => None
        })
    })
        .collect::<Result<Vec<Option<(u32, Vec<proto::Accuracy>)>>>>()?
        .into_iter().filter_map(|v| v)
        .collect::<HashMap<u32, Vec<proto::Accuracy>>>();

    Ok(proto::Accuracies {
        values: accuracies.into_iter().map(|(_, v)| v).collect::<Vec<Vec<proto::Accuracy>>>()
            // TODO: propagate/combine accuracies, don't just take the first
            .first()
            .ok_or_else(|| Error::from("accuracy is not defined"))?.clone()
    })
}

/// Retrieve the static properties from every reachable node on the graph.
pub fn get_properties(
    mut analysis: proto::Analysis,
    mut release: base::Release,
    node_ids: Vec<u32>
) -> Result<proto::GraphProperties> {

    if node_ids.len() > 0 {
        let mut ancestors = HashSet::<u32>::new();
        let mut traversal = Vec::from_iter(node_ids.into_iter());
        let computation_graph = &analysis.computation_graph.as_ref().unwrap().value;
        while !traversal.is_empty() {
            let node_id = traversal.pop().unwrap();
            computation_graph.get(&node_id)
                .map(|component| component.arguments.values().for_each(|v| traversal.push(*v)));
            ancestors.insert(node_id);
        }
        analysis = proto::Analysis {
            computation_graph: Some(proto::ComputationGraph {
                value: computation_graph.iter()
                    .filter(|(idx, _)| ancestors.contains(idx))
                    .map(|(idx, component)| (idx.clone(), component.clone()))
                    .collect::<HashMap<u32, proto::Component>>()
            }),
            privacy_definition: analysis.privacy_definition,
        };
        release = release.iter()
            .filter(|(idx, _)| ancestors.contains(idx))
            .map(|(idx, release_node)|
                (idx.clone(), release_node.clone()))
            .collect();
    }

    let (properties, warnings) = utilities::propagate_properties(
        &mut analysis, &mut release, None, true,
    )?;

    Ok(proto::GraphProperties {
        properties: properties.into_iter()
            .map(|(node_id, properties)| (node_id, serialize_value_properties(properties)))
            .collect::<HashMap<u32, proto::ValueProperties>>(),
        warnings,
    })
}


/// Expand a component that may be representable as smaller components, and propagate its properties.
///
/// This is function may be called interactively from the runtime as the runtime executes the computational graph, to allow for dynamic graph validation.
/// This is opposed to statically validating a graph, where the nodes in the graph that are dependent on the releases of mechanisms cannot be known and validated until the first release is made.
pub fn expand_component(
    request: proto::RequestExpandComponent
) -> Result<proto::ComponentExpansion> {
    let proto::RequestExpandComponent {
        component, properties, arguments, privacy_definition, component_id, maximum_id,
    } = request;

    let public_arguments = arguments.into_iter()
        .map(|(k, v)| (k, utilities::serial::parse_release_node(v)))
        .collect::<HashMap<String, ReleaseNode>>();

    let mut properties: base::NodeProperties = properties.into_iter()
        .map(|(k, v)| (k, utilities::serial::parse_value_properties(v)))
        .collect();

    for (k, v) in &public_arguments {
        // this if should be redundant, no private data should be passed to the validator
        if v.public {
            properties.insert(k.clone(), utilities::inference::infer_property(&v.value, None)?);
        }
    }

    let component = component
        .ok_or_else(|| Error::from("component must be defined"))?;

    let mut result = component.expand_component(
        &privacy_definition,
        &component,
        &properties,
        &component_id,
        &maximum_id,
    ).chain_err(|| format!("at node_id {:?}", component_id))?;

    let public_values = public_arguments.into_iter()
        .map(|(name, release_node)| (name.clone(), release_node.value.clone()))
        .collect::<HashMap<String, Value>>();

    if result.traversal.is_empty() {
        let Warnable(propagated_property, propagation_warnings) = component.clone()
            .propagate_property(&privacy_definition, &public_values, &properties, component_id)
            .chain_err(|| format!("at node_id {:?}", component_id))?;

        result.warnings.extend(propagation_warnings.into_iter()
            .map(|err| serialize_error(err.chain_err(|| format!("at node_id {:?}", component_id)))));
        result.properties.insert(component_id.to_owned(), utilities::serial::serialize_value_properties(propagated_property));
    }

    Ok(result)
}



impl Div<f64> for proto::PrivacyUsage {
    type Output = Result<proto::PrivacyUsage>;

    fn div(mut self, rhs: f64) -> Self::Output {
        self.distance = Some(match self.distance.ok_or_else(|| "distance must be defined")? {
            proto::privacy_usage::Distance::Approximate(approximate) => proto::privacy_usage::Distance::Approximate(proto::privacy_usage::DistanceApproximate {
                epsilon: approximate.epsilon / rhs,
                delta: approximate.delta / rhs,
            })
        });
        Ok(self)
    }
}

impl Mul<f64> for proto::PrivacyUsage {
    type Output = Result<proto::PrivacyUsage>;

    fn mul(mut self, rhs: f64) -> Self::Output {
        self.distance = Some(match self.distance.ok_or_else(|| "distance must be defined")? {
            proto::privacy_usage::Distance::Approximate(approximate) => proto::privacy_usage::Distance::Approximate(proto::privacy_usage::DistanceApproximate {
                epsilon: approximate.epsilon * rhs,
                delta: approximate.delta * rhs,
            })
        });
        Ok(self)
    }
}

impl Add<proto::PrivacyUsage> for proto::PrivacyUsage {
    type Output = Result<proto::PrivacyUsage>;

    fn add(mut self, rhs: proto::PrivacyUsage) -> Self::Output {
        let left_distance = self.distance.ok_or_else(|| "distance must be defined")?;
        let right_distance = rhs.distance.ok_or_else(|| "distance must be defined")?;

        use proto::privacy_usage::Distance;

        self.distance = Some(match (left_distance, right_distance) {
            (Distance::Approximate(lhs), Distance::Approximate(rhs)) => proto::privacy_usage::Distance::Approximate(proto::privacy_usage::DistanceApproximate {
                epsilon: lhs.epsilon + rhs.epsilon,
                delta: lhs.delta + rhs.delta,
            })
        });
        Ok(self)
    }
}