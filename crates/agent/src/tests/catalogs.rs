use super::*;

#[test]
fn system_prompt_specifies_formal_french_and_byte_units() {
    assert!(SYSTEM.contains("« vous »"));
    assert!(SYSTEM.contains("Gio"));
    assert!(SYSTEM.contains("never Go, Mo, GiB or MiB"));
    assert!(ELEVATED.contains("call get_approved_elevated_inspection"));
}

#[test]
fn instructions_name_the_elevated_tool_only_when_it_is_declared() {
    use crate::capabilities::{catalog, registry, Capabilities, Capability};
    let prompt = |capabilities: &Capabilities| {
        system_prompt(
            &catalog(&registry(), capabilities),
            Sharing::Complete,
            false,
            None,
        )
    };
    // With every capability the instructions are the original ones, in their original order.
    let full = prompt(&Capabilities::all());
    assert!(full.starts_with(&format!(
        "{SYSTEM}{ELEVATED}{SYSTEM_RULES}{CURRENT_FINDINGS}"
    )));
    let desktop = prompt(&Capabilities::all().with(Capability::Battery, false));
    assert!(desktop.contains(ELEVATED_WITHOUT_BATTERY));
    assert!(!desktop.contains("energy apps used on battery"));
    for capabilities in [
        Capabilities::all().with(Capability::ElevatedHelper, false),
        Capabilities::all().with(Capability::Windows, false),
        Capabilities::none(),
    ] {
        let text = prompt(&capabilities);
        assert!(text.contains(NO_ELEVATED), "{capabilities:?}");
        assert!(!text.contains("get_approved_elevated_inspection"));
    }
    // Explain declares only the frozen evidence.
    let explain = system_prompt(
        &[selection::GET_SELECTED_EVIDENCE],
        Sharing::Anonymized,
        false,
        Some(selection::SelectionIntent::Explain),
    );
    assert!(!explain.contains("get_approved_elevated_inspection"));
    assert!(explain.contains(NO_ELEVATED));
}

#[test]
fn the_prompt_and_answer_schema_name_only_declared_tools() {
    use crate::capabilities::{catalog, every_combination, names_tool, registry};
    let all: Vec<&str> = registry().iter().map(|s| s.name).collect();
    for capabilities in every_combination() {
        let declared = catalog(&registry(), &capabilities);
        let names: Vec<&str> = declared.iter().map(|s| s.name).collect();
        let proposals = names.contains(&PREPARE_ACTION_PLAN.name);
        for (follow_up, selected) in [
            (false, None),
            (true, Some(selection::SelectionIntent::Investigate)),
        ] {
            let texts = [
                system_prompt(&declared, Sharing::Complete, follow_up, selected),
                answer::definition_for(proposals).to_string(),
            ];
            for text in &texts {
                for absent in all.iter().filter(|name| !names.contains(name)) {
                    assert!(
                        !names_tool(text, absent),
                        "{absent} named without being declared ({capabilities:?})"
                    );
                }
            }
        }
    }
}

#[test]
fn a_turn_sends_its_declared_tools_and_the_matching_answer_schema() {
    use crate::capabilities::{catalog, registry, Capabilities, Capability};
    struct Recording(Vec<Value>);
    impl Model for Recording {
        fn complete(
            &mut self,
            _: &[Value],
            tools: &Value,
            _: ToolChoice,
            _: Duration,
        ) -> Result<Reply, AgentError> {
            self.0.push(tools.clone());
            Err(AgentError::Network)
        }
    }
    struct Declared(Vec<ToolSpec>);
    impl Toolbox for Declared {
        fn specs(&self) -> &[ToolSpec] {
            &self.0
        }
        fn run(&mut self, _: &ToolSpec, _: &Args) -> ToolOutput {
            unreachable!("the model fails before any tool call")
        }
    }
    for capabilities in [
        Capabilities::all(),
        Capabilities::all()
            .with(Capability::ActionProposals, false)
            .with(Capability::ElevatedHelper, false)
            .with(Capability::StorageScopes, false),
        Capabilities::none(),
    ] {
        let declared = catalog(&registry(), &capabilities);
        let mut model = Recording(Vec::new());
        investigate(
            &mut model,
            "Why is my PC slow?",
            &fixture(),
            &mut Declared(declared.clone()),
            |_| {},
        );
        let sent = model.0[0].as_array().unwrap().clone();
        assert_eq!(sent.len(), declared.len() + 1);
        for (definition, spec) in sent.iter().zip(&declared) {
            assert_eq!(*definition, spec.definition());
        }
        let submit = sent.last().unwrap();
        let action_refs = submit["function"]["parameters"]["properties"]["action_refs"]
            ["description"]
            .as_str()
            .unwrap();
        assert_eq!(
            action_refs.contains("prepare_action_plan"),
            capabilities.contains(Capability::ActionProposals)
                && capabilities.contains(Capability::Windows)
        );
        // Only that description changes: the answer schema stays the same (C1).
        let schema = |definition: &Value| {
            let mut definition = definition.clone();
            definition["function"]["parameters"]["properties"]["action_refs"]["description"] =
                json!("");
            definition
        };
        assert_eq!(schema(submit), schema(&answer::definition()));
    }
}

#[test]
fn instructions_mention_proposals_only_when_the_tool_is_offered() {
    let plain = system_prompt(
        SnapshotTools::new(|_, _| fixture()).specs(),
        Sharing::Anonymized,
        false,
        None,
    );
    assert!(plain.ends_with(NO_ACTIONS));
    assert!(!plain.contains("prepare_action_plan"));
    let with_plans = system_prompt(&[PREPARE_ACTION_PLAN], Sharing::Complete, true, None);
    assert!(with_plans.contains("previous_turns"));
    assert!(!plain.contains("Earlier questions"));
    assert!(with_plans.contains("process_names"));
    assert!(plain.contains("do not guess"));
    assert!(with_plans.contains("call prepare_action_plan"));
    assert!(!with_plans.contains(NO_ACTIONS));
}
