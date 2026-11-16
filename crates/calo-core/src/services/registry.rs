//! Three-layer service registry, options, expression/Python wrappers, and dependency graph (spec §12.2).

use crate::expr::{eval, parse, references, Value};
use crate::services::builtins::{
    CacheMissRateService, ImbalanceService, IpcService, ParallelEfficiencyService, SpeedupService,
    StatsSummaryService,
};
use crate::services::error::ServiceError;
use crate::services::frame::{Column, Frame};
use crate::services::Service;
use calo_schema::suite::{CiConfig, OutlierConfig, ScalingKind, ScalingOver, ServiceConfig};
use calo_schema::{ScalarValue, Tier};
use std::collections::{BTreeMap, BTreeSet};

/// Service customization layer (lowest to highest: BuiltIn < Suite < Report, spec §12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceLayer {
    /// Built-in Rust service library.
    BuiltIn,
    /// Suite-level `services:` block configuration/definition.
    Suite,
    /// Report-local service override.
    Report,
}

/// Runtime options passed to `Service::run` (spec §12.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Target input metric column name (defaults to `"wall_s"` for `stats.summary`, `speedup`, etc.).
    pub metric: String,
    /// Estimator for `stats.summary` (`"median"` or `"mean"`, default `"median"`).
    pub estimator: String,
    /// Bootstrap CI configuration (default: level `0.95`, resamples `10000`, seed `1`).
    pub ci: CiConfig,
    /// MAD outlier filter configuration (optional; enabled when `Some`).
    pub outliers: Option<OutlierConfig>,
    /// Scaling kind (`Strong` or `Weak`) for `parallel_efficiency`.
    pub scaling_kind: ScalingKind,
    /// Resource column name for `parallel_efficiency` (`"nodes"`, `"ranks"`, or `"gpus"`).
    pub scaling_over: String,
    /// Explicit baseline parameter/row filter for `speedup` and `parallel_efficiency`.
    pub baseline: BTreeMap<String, ScalarValue>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            metric: "wall_s".to_string(),
            estimator: "median".to_string(),
            ci: CiConfig {
                method: "bootstrap".to_string(),
                level: 0.95,
                resamples: 1000,
                seed: 1,
            },
            outliers: None,
            scaling_kind: ScalingKind::Strong,
            scaling_over: "ranks".to_string(),
            baseline: BTreeMap::new(),
        }
    }
}

impl Options {
    /// Applies overrides from a `calo_schema::suite::ServiceConfig` onto these `Options`.
    pub fn apply_service_config(&mut self, cfg: &ServiceConfig) {
        if let Some(est) = &cfg.estimator {
            self.estimator.clone_from(est);
        }
        if let Some(ci) = &cfg.ci {
            self.ci = ci.clone();
        }
        if let Some(out) = &cfg.outliers {
            self.outliers = Some(out.clone());
        }
        if let Some(first_input) = cfg.inputs.first() {
            self.metric.clone_from(first_input);
        }
    }

    /// Sets `scaling_over` from `ScalingOver`.
    pub fn with_scaling_over(mut self, over: ScalingOver) -> Self {
        self.scaling_over = match over {
            ScalingOver::Nodes => "nodes".to_string(),
            ScalingOver::Ranks => "ranks".to_string(),
            ScalingOver::Gpus => "gpus".to_string(),
        };
        self
    }
}

/// Expression-defined service (`expr:` + `unit:`, spec §12.2 R-SVC-5).
pub struct ExpressionService {
    name: &'static str,
    expr_src: String,
    unit: Option<String>,
    inputs: &'static [&'static str],
    produces: &'static [&'static str],
    requires: &'static [Tier],
}

impl ExpressionService {
    /// Constructs a new `ExpressionService` from a service name and `ServiceConfig`.
    pub fn from_config(name: &str, cfg: &ServiceConfig) -> Result<Self, ServiceError> {
        let expr_src = cfg
            .expr
            .clone()
            .ok_or_else(|| ServiceError::InvalidInput {
                service: name.to_string(),
                reason: "missing `expr` field".to_string(),
            })?;
        let ast = parse(&expr_src).map_err(|source| ServiceError::Expression {
            service: name.to_string(),
            source,
        })?;

        let raw_inputs: Vec<String> = if cfg.inputs.is_empty() {
            references(&ast).into_iter().collect()
        } else {
            cfg.inputs.clone()
        };
        let raw_produces: Vec<String> = if cfg.produces.is_empty() {
            vec![name.to_string()]
        } else {
            cfg.produces.clone()
        };

        Ok(Self {
            name: leak_str(name),
            expr_src,
            unit: cfg.unit.clone(),
            inputs: leak_str_slice(&raw_inputs),
            produces: leak_str_slice(&raw_produces),
            requires: leak_tier_slice(&cfg.requires),
        })
    }
}

impl Service for ExpressionService {
    fn name(&self) -> &'static str {
        self.name
    }

    fn inputs(&self) -> &'static [&'static str] {
        self.inputs
    }

    fn produces(&self) -> &'static [&'static str] {
        self.produces
    }

    fn requires(&self) -> &'static [Tier] {
        self.requires
    }

    fn run(&self, table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        let ast = parse(&self.expr_src).map_err(|source| ServiceError::Expression {
            service: self.name.to_string(),
            source,
        })?;

        let mut out = table.clone();
        let mut results = Vec::with_capacity(table.num_rows());
        for row_idx in 0..table.num_rows() {
            let env = table.row_env(row_idx);
            let val = eval(&ast, &env).map_err(|source| ServiceError::Expression {
                service: self.name.to_string(),
                source,
            })?;
            let f = val.as_f64().ok_or_else(|| ServiceError::InvalidInput {
                service: self.name.to_string(),
                reason: format!(
                    "expression must evaluate to a number, got {}",
                    val.type_name()
                ),
            })?;
            results.push(f);
        }

        let out_col_name = self.produces.first().copied().unwrap_or(self.name);
        out.insert_column(out_col_name, Column::Float(results))?;
        if let Some(u) = &self.unit {
            out.set_meta(format!("{out_col_name}.unit"), ScalarValue::Str(u.clone()));
        }
        Ok(out)
    }
}

/// Stub service for external Python definitions returning `ServiceError::PythonUnsupported` (spec §12.4).
pub struct PythonStubService {
    name: &'static str,
    entrypoint: String,
    inputs: &'static [&'static str],
    produces: &'static [&'static str],
    requires: &'static [Tier],
}

impl PythonStubService {
    /// Constructs a `PythonStubService` from a `ServiceConfig` with `python:`.
    pub fn from_config(name: &str, cfg: &ServiceConfig, entrypoint: &str) -> Self {
        Self {
            name: leak_str(name),
            entrypoint: entrypoint.to_string(),
            inputs: leak_str_slice(&cfg.inputs),
            produces: leak_str_slice(&cfg.produces),
            requires: leak_tier_slice(&cfg.requires),
        }
    }
}

impl Service for PythonStubService {
    fn name(&self) -> &'static str {
        self.name
    }

    fn inputs(&self) -> &'static [&'static str] {
        self.inputs
    }

    fn produces(&self) -> &'static [&'static str] {
        self.produces
    }

    fn requires(&self) -> &'static [Tier] {
        self.requires
    }

    fn run(&self, _table: &Frame, _opts: &Options) -> Result<Frame, ServiceError> {
        Err(ServiceError::PythonUnsupported {
            service: self.name.to_string(),
            entrypoint: self.entrypoint.clone(),
        })
    }
}

/// Three-layer registry of services (`BuiltIn`, `Suite`, `Report`, spec §12.2).
pub struct ServiceRegistry {
    services: BTreeMap<String, (ServiceLayer, Box<dyn Service>)>,
    options: BTreeMap<String, (ServiceLayer, Options)>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    /// Creates a new `ServiceRegistry` pre-populated with all built-in Rust services (spec §12.3).
    pub fn new() -> Self {
        let mut reg = Self {
            services: BTreeMap::new(),
            options: BTreeMap::new(),
        };
        reg.register_builtin(Box::new(StatsSummaryService));
        reg.register_builtin(Box::new(IpcService));
        reg.register_builtin(Box::new(CacheMissRateService));
        reg.register_builtin(Box::new(SpeedupService));
        reg.register_builtin(Box::new(ParallelEfficiencyService));
        reg.register_builtin(Box::new(ImbalanceService));
        reg
    }

    fn register_builtin(&mut self, svc: Box<dyn Service>) {
        let name = svc.name().to_string();
        self.services.insert(name.clone(), (ServiceLayer::BuiltIn, svc));
        self.options
            .insert(name, (ServiceLayer::BuiltIn, Options::default()));
    }

    /// Applies a map of `ServiceConfig` entries at the given `layer` (`Suite` or `Report`, spec §12.2).
    ///
    /// Supports configuring existing built-ins, overriding existing services, and defining new
    /// expression or Python services.
    pub fn apply_layer(
        &mut self,
        layer: ServiceLayer,
        configs: &BTreeMap<String, ServiceConfig>,
    ) -> Result<(), ServiceError> {
        for (name, cfg) in configs {
            if let Some(py_entry) = &cfg.python {
                let stub = PythonStubService::from_config(name, cfg, py_entry);
                self.services.insert(name.clone(), (layer, Box::new(stub)));
                let mut opts = self
                    .options
                    .get(name)
                    .map(|(_, o)| o.clone())
                    .unwrap_or_default();
                opts.apply_service_config(cfg);
                self.options.insert(name.clone(), (layer, opts));
            } else if cfg.expr.is_some() {
                let expr_svc = ExpressionService::from_config(name, cfg)?;
                self.services
                    .insert(name.clone(), (layer, Box::new(expr_svc)));
                let mut opts = self
                    .options
                    .get(name)
                    .map(|(_, o)| o.clone())
                    .unwrap_or_default();
                opts.apply_service_config(cfg);
                self.options.insert(name.clone(), (layer, opts));
            } else if self.services.contains_key(name) {
                // Configure an existing service at a higher precedence layer
                let mut opts = self
                    .options
                    .get(name)
                    .map(|(_, o)| o.clone())
                    .unwrap_or_default();
                opts.apply_service_config(cfg);
                self.options.insert(name.clone(), (layer, opts));
            } else {
                return Err(ServiceError::UnknownService { name: name.clone() });
            }
        }
        Ok(())
    }

    /// Looks up a registered `Service` by name.
    pub fn get(&self, name: &str) -> Option<&dyn Service> {
        self.services.get(name).map(|(_, s)| s.as_ref())
    }

    /// Returns the resolved `Options` for `name`.
    pub fn options_for(&self, name: &str) -> Options {
        self.options
            .get(name)
            .map(|(_, o)| o.clone())
            .unwrap_or_default()
    }

    /// Returns the winning `ServiceLayer` for `name`.
    pub fn layer_for(&self, name: &str) -> Option<ServiceLayer> {
        self.options
            .get(name)
            .map(|(l, _)| *l)
            .or_else(|| self.services.get(name).map(|(l, _)| *l))
    }

    /// Executes a named service using its layered options.
    pub fn run_service(&self, name: &str, frame: &Frame) -> Result<Frame, ServiceError> {
        let svc = self
            .get(name)
            .ok_or_else(|| ServiceError::UnknownService {
                name: name.to_string(),
            })?;
        let opts = self.options_for(name);
        svc.run(frame, &opts)
    }

    /// Builds the dependency graph over all registered services and returns their topological execution order,
    /// rejecting any dependency cycle with `ServiceError::DependencyCycle` (spec §12.2 R-SVC-2, R-SVC-4).
    pub fn execution_order(&self) -> Result<Vec<&'static str>, ServiceError> {
        // Map each produced column -> producing service name
        let mut producer_of: BTreeMap<&str, &str> = BTreeMap::new();
        for (svc_name, (_, svc)) in &self.services {
            producer_of.insert(svc_name.as_str(), svc.name());
            for &prod in svc.produces() {
                producer_of.insert(prod, svc.name());
            }
        }

        let mut deps: BTreeMap<&'static str, BTreeSet<&'static str>> = BTreeMap::new();
        for (_, svc) in self.services.values() {
            let mut d = BTreeSet::new();
            for &inp in svc.inputs() {
                if let Some(&prod_svc) = producer_of.get(inp) {
                    if prod_svc != svc.name() {
                        d.insert(prod_svc);
                    }
                }
            }
            deps.insert(svc.name(), d);
        }

        let mut sorted = Vec::with_capacity(self.services.len());
        let mut resolved = BTreeSet::new();
        let mut remaining: BTreeSet<&'static str> = deps.keys().copied().collect();

        while !remaining.is_empty() {
            let ready: Vec<&'static str> = remaining
                .iter()
                .filter(|&&k| {
                    deps[k]
                        .iter()
                        .all(|&dep| !remaining.contains(dep) || resolved.contains(dep))
                })
                .copied()
                .collect();

            if ready.is_empty() {
                let cycle_names: Vec<&str> = remaining.into_iter().collect();
                return Err(ServiceError::DependencyCycle {
                    cycle: cycle_names.join(" -> "),
                });
            }

            for k in ready {
                remaining.remove(k);
                resolved.insert(k);
                sorted.push(k);
            }
        }

        Ok(sorted)
    }
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn leak_str_slice(items: &[String]) -> &'static [&'static str] {
    let vec: Vec<&'static str> = items.iter().map(|s| leak_str(s)).collect();
    Box::leak(vec.into_boxed_slice())
}

fn leak_tier_slice(items: &[Tier]) -> &'static [Tier] {
    let vec: Vec<Tier> = items.to_vec();
    Box::leak(vec.into_boxed_slice())
}
