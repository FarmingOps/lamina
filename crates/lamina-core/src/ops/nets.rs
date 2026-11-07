use std::collections::BTreeMap;

use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetNets {
    /// A map from the net name to its pads and an optional net class.
    pub nets: BTreeMap<String, Net>,
    /// If true (default), replace the full netlist. If false, merge into it.
    #[serde(default = "super::yes")]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Connect {
    pub net: String,
    pub pads: Vec<PadRef>,
    /// Set or change the net class.
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Disconnect {
    pub net: String,
    /// The pads to remove. If you do not give pads, the net is deleted.
    #[serde(default)]
    pub pads: Option<Vec<PadRef>>,
    #[serde(default)]
    pub strict: bool,
}

fn check_name(n: &str) -> Result<()> {
    if valid_net_name(n) {
        Ok(())
    } else {
        Err(Error::invalid(format!("The net name `{n}` is not valid.")))
    }
}

/// Refuse a pad that cannot exist. That is, the part is known and has a
/// footprint, but the footprint does not have this pad.
fn check_pads(d: &ProjectData, pads: &[PadRef]) -> Result<()> {
    for p in pads {
        if let Some(part) = d.parts.parts.get(&p.part) {
            if let Some(fp) = d.footprints.get(&part.footprint) {
                if fp.pad(&p.pad).is_none() {
                    let mut names: Vec<&str> = fp.pads.iter().map(|x| x.number.as_str()).collect();
                    names.sort();
                    names.dedup();
                    return Err(Error::invalid(format!(
                        "{p}: the footprint `{}` has no pad `{}`. Its pads are: {}.",
                        fp.id,
                        p.pad,
                        names.join(" ")
                    )));
                }
            }
        }
    }
    Ok(())
}

impl Project {
    pub fn set_nets(&mut self, p: SetNets) -> Result<OpResult> {
        let mut d = self.data.clone();
        for (name, net) in &p.nets {
            check_name(name)?;
            check_pads(&d, &net.pads)?;
        }
        let count = p.nets.len();
        if p.replace {
            d.nets.nets = p.nets;
        } else {
            for (k, mut v) in p.nets {
                v.pads.sort();
                v.pads.dedup();
                d.nets.nets.insert(k, v);
            }
        }
        for net in d.nets.nets.values_mut() {
            net.pads.sort();
            net.pads.dedup();
        }
        let verb = if p.replace { "set" } else { "merged" };
        self.commit(d, Touched::file(FileKind::Nets), p.strict, "")
            .map(|mut r| {
                r.summary = format!(
                    "{verb} {count} net(s). The netlist has {} nets.",
                    self.data.nets.nets.len()
                );
                r
            })
    }

    pub fn connect(&mut self, p: Connect) -> Result<OpResult> {
        check_name(&p.net)?;
        if p.pads.is_empty() && p.class.is_none() {
            return Err(Error::invalid(
                "No pad and no net class are given. Give at least one pad or a net class.",
            ));
        }
        let mut d = self.data.clone();
        check_pads(&d, &p.pads)?;
        let net = d.nets.nets.entry(p.net.clone()).or_default();
        net.pads.extend(p.pads.iter().cloned());
        net.pads.sort();
        net.pads.dedup();
        if p.class.is_some() {
            net.class = p.class.filter(|c| !c.is_empty());
        }
        let n = net.pads.len();
        self.commit(
            d,
            Touched::file(FileKind::Nets),
            p.strict,
            format!("net {} now has {n} pad(s)", p.net),
        )
    }

    pub fn disconnect(&mut self, p: Disconnect) -> Result<OpResult> {
        let mut d = self.data.clone();
        if !d.nets.nets.contains_key(&p.net) {
            return Err(Error::invalid(format!(
                "The net `{}` does not exist.",
                p.net
            )));
        }
        let summary = match p.pads {
            None => {
                d.nets.nets.remove(&p.net);
                format!("net {} removed", p.net)
            }
            Some(pads) => {
                let net = d.nets.nets.get_mut(&p.net).unwrap();
                let missing: Vec<String> = pads
                    .iter()
                    .filter(|x| !net.pads.contains(x))
                    .map(|x| x.to_string())
                    .collect();
                if !missing.is_empty() {
                    return Err(Error::invalid(format!(
                        "These pads are not in the net {}: {}.",
                        p.net,
                        missing.join(", ")
                    )));
                }
                net.pads.retain(|x| !pads.contains(x));
                format!("net {} now has {} pad(s)", p.net, net.pads.len())
            }
        };
        self.commit(d, Touched::file(FileKind::Nets), p.strict, summary)
    }
}
