//! The branch drops every key name a config parse error quotes, because "a key
//! name can be a value pasted in the wrong place". The start checks, which
//! the terminal UI refuses with and `dux config get`/`set` print, still quote
//! a key name taken from the file when the problem is about that very name.

#[test]
fn a_token_pasted_as_an_env_name_is_quoted_by_the_start_refusal() {
    let token = "sk-proj-AbCdEf0123456789";
    let raw = format!("[env]\n{token} = \"1\"\n");
    let problems = dux_core::config::start_problems_of(&raw);
    assert!(
        !problems.is_empty(),
        "precondition: the name stops the terminal UI"
    );
    let leaked: Vec<&String> = problems
        .iter()
        .map(|p| &p.message)
        .filter(|m| m.contains(token))
        .collect();
    assert!(
        leaked.is_empty(),
        "start refusal quotes the pasted token: {leaked:?}"
    );
}
