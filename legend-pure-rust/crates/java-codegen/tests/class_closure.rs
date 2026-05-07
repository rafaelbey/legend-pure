// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod common;

use legend_pure_java_codegen::{FqnInput, Options, generate};

const SOURCE: &str = r#"
Class user_test::Address
{
    street: String[1];
}

Class user_test::Person
{
    firstName: String[1];
    age: Integer[0..1];
    addresses: user_test::Address[*];
}

function user_test::makePerson(firstName: String[1]): user_test::Person[1]
{
    ^user_test::Person(firstName=$firstName, addresses=[]);
}
"#;

#[test]
fn class_closure_emits_person_and_address_interfaces() {
    let model = common::compile_with_platform(Some(SOURCE));
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new("user_test::makePerson_String_1__Person_1_")];
    let files = generate(&model, &fns, &[], &[], &opts).expect("codegen succeeds");

    let person = files
        .iter()
        .find(|f| f.relative_path.ends_with("Person.java"))
        .expect("Person.java emitted");
    let address = files
        .iter()
        .find(|f| f.relative_path.ends_with("Address.java"))
        .expect("Address.java emitted (must be reachable from Person)");
    let facade = files
        .iter()
        .find(|f| f.relative_path.ends_with("PureFunctions.java"))
        .expect("PureFunctions.java emitted");

    let person_src = &person.contents;
    assert!(
        person_src.contains("package com.example.gen.user_test;"),
        "Person package wrong: {person_src}"
    );
    assert!(
        person_src.contains("public interface Person extends "),
        "Person should be an interface with extends clause: {person_src}"
    );
    assert!(
        person_src.contains("String firstName();"),
        "Person.firstName() missing: {person_src}"
    );
    assert!(
        person_src.contains("java.util.Optional<Long> age();"),
        "Person.age() must be Optional<Long> for Integer[0..1]: {person_src}"
    );
    assert!(
        person_src.contains("Iterable<com.example.gen.user_test.Address> addresses();"),
        "Person.addresses() must reference fully-qualified Address: {person_src}"
    );
    assert!(
        person_src.contains("PureProxyFactory.register(\"user_test::Person\", Person.class)"),
        "Person must register itself with the proxy factory: {person_src}"
    );

    let address_src = &address.contents;
    assert!(
        address_src.contains("public interface Address extends "),
        "Address must be an interface: {address_src}"
    );
    assert!(
        address_src.contains("String street();"),
        "Address.street() missing: {address_src}"
    );

    let facade_src = &facade.contents;
    assert!(
        facade_src.contains("static {"),
        "facade must have a static init block to force interface registration: {facade_src}"
    );
    assert!(
        facade_src.contains("com.example.gen.user_test.Person.$REGISTERED")
            && facade_src.contains("com.example.gen.user_test.Address.$REGISTERED"),
        "facade must touch every reachable interface: {facade_src}"
    );
    assert!(
        facade_src.contains(
            "public static com.example.gen.user_test.Person user_test_makePerson_String_1__Person_1_("
        ),
        "facade must declare the typed wrapper: {facade_src}"
    );
}
