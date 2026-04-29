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

use jni::JNIEnv;
use jni::objects::JObject;
use legend_pure_runtime::value::Value;

/// Convert a Rust Value into a Java RustResult object
pub fn rust_to_java_result<'local>(
    env: &mut JNIEnv<'local>,
    value: &Value,
    context_ptr: i64,
) -> Result<JObject<'local>, jni::errors::Error> {
    let cls = env.find_class("org/finos/legend/pure/rust/PureRustResult")?;
    let type_cls = env.find_class("org/finos/legend/pure/rust/PureRustResult$Type")?;

    match value {
        Value::Integer(i) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "INTEGER",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let val_obj =
                env.new_object("java/lang/Long", "(J)V", &[jni::objects::JValue::Long(*i)])?;

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&val_obj).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Boolean(b) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "BOOLEAN",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let val_obj = env.new_object(
                "java/lang/Boolean",
                "(Z)V",
                &[jni::objects::JValue::Bool((*b).into())],
            )?;

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&val_obj).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Float(f) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "FLOAT",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let val_obj = env.new_object(
                "java/lang/Double",
                "(D)V",
                &[jni::objects::JValue::Double(*f)],
            )?;

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&val_obj).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Unit => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "NULL",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&JObject::null()).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::String(s) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "STRING",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let val_string = env.new_string(s.as_str())?;
            let val_obj: jni::objects::JObject = val_string.into();

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&val_obj).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Object(obj_id) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "INSTANCE_POINTER",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let ptr_val: u64 = slotmap::Key::data(obj_id).as_ffi();
            let val_obj = env.new_object(
                "java/lang/Long",
                "(J)V",
                &[jni::objects::JValue::Long(ptr_val as i64)],
            )?;

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&val_obj).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Collection(vec) => {
            let enum_val = env
                .get_static_field(
                    &type_cls,
                    "ARRAY",
                    "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                )?
                .l()?;
            let array = env.new_object_array(vec.len() as i32, &cls, JObject::null())?;
            for (i, v) in vec.iter().enumerate() {
                let item = rust_to_java_result(env, v, context_ptr)?;
                env.set_object_array_element(&array, i as i32, &item)?;
            }

            let method_id = env.get_method_id(
                &cls,
                "<init>",
                "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
            )?;
            unsafe {
                env.new_object_unchecked(
                    &cls,
                    method_id,
                    &[
                        jni::objects::JValue::Object(&enum_val).as_jni(),
                        jni::objects::JValue::Object(&JObject::from(array)).as_jni(),
                        jni::objects::JValue::Long(context_ptr).as_jni(),
                    ],
                )
            }
        }
        Value::Element(element_id) => {
            let context = unsafe { &*(context_ptr as *mut crate::context::JniContext) };
            if let Some(obj_id) = context.object_for_element(*element_id) {
                let enum_val = env
                    .get_static_field(
                        &type_cls,
                        "INSTANCE_POINTER",
                        "Lorg/finos/legend/pure/rust/PureRustResult$Type;",
                    )?
                    .l()?;
                let ptr_val: u64 = slotmap::Key::data(&obj_id).as_ffi();
                let val_obj = env.new_object(
                    "java/lang/Long",
                    "(J)V",
                    &[jni::objects::JValue::Long(ptr_val as i64)],
                )?;

                let method_id = env.get_method_id(
                    &cls,
                    "<init>",
                    "(Lorg/finos/legend/pure/rust/PureRustResult$Type;Ljava/lang/Object;J)V",
                )?;
                unsafe {
                    env.new_object_unchecked(
                        &cls,
                        method_id,
                        &[
                            jni::objects::JValue::Object(&enum_val).as_jni(),
                            jni::objects::JValue::Object(&val_obj).as_jni(),
                            jni::objects::JValue::Long(context_ptr).as_jni(),
                        ],
                    )
                }
            } else {
                let _ = env.throw_new(
                    "org/finos/legend/pure/rust/PureRustEvaluationException",
                    format!("Element has no object representation: {:?}", element_id),
                );
                Ok(JObject::null())
            }
        }
        _ => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustEvaluationException",
                format!("Unsupported value type: {:?}", value),
            );
            Ok(JObject::null())
        }
    }
}

pub fn java_to_rust_value<'local>(
    env: &mut JNIEnv<'local>,
    obj: &JObject<'local>,
    context_ptr: i64,
) -> Result<Value, jni::errors::Error> {
    if obj.is_null() {
        return Ok(Value::Unit);
    }

    let type_enum = env
        .call_method(
            obj,
            "getType",
            "()Lorg/finos/legend/pure/rust/PureRustResult$Type;",
            &[],
        )?
        .l()?;
    let name_str_obj = env
        .call_method(&type_enum, "name", "()Ljava/lang/String;", &[])?
        .l()?;
    let name_string: String = env.get_string((&name_str_obj).into())?.into();

    match name_string.as_str() {
        "INTEGER" => {
            let val = env
                .call_method(obj, "getAsInteger", "()Ljava/lang/Long;", &[])?
                .l()?;
            let primitive = env.call_method(&val, "longValue", "()J", &[])?.j()?;
            Ok(Value::Integer(primitive))
        }
        "STRING" => {
            let val = env
                .call_method(obj, "getAsString", "()Ljava/lang/String;", &[])?
                .l()?;
            let s: String = env.get_string((&val).into())?.into();
            Ok(Value::String(smol_str::SmolStr::new(s)))
        }
        "BOOLEAN" => {
            let val = env
                .call_method(obj, "getAsBoolean", "()Ljava/lang/Boolean;", &[])?
                .l()?;
            let primitive = env.call_method(&val, "booleanValue", "()Z", &[])?.z()?;
            Ok(Value::Boolean(primitive))
        }
        "FLOAT" => {
            let val = env
                .call_method(obj, "getAsFloat", "()Ljava/lang/Double;", &[])?
                .l()?;
            let primitive = env.call_method(&val, "doubleValue", "()D", &[])?.d()?;
            Ok(Value::Float(primitive))
        }
        "INSTANCE_POINTER" => {
            let val = env
                .call_method(obj, "getAsInstancePointer", "()J", &[])?
                .j()?;
            let key = slotmap::KeyData::from_ffi(val as u64);
            let obj_id = legend_pure_runtime::heap::ObjectId::from(key);

            let context = unsafe { &*(context_ptr as *mut crate::context::JniContext) };
            if let Some(element_id) = context.element_for_object(obj_id) {
                Ok(Value::Element(element_id))
            } else {
                Ok(Value::Object(obj_id))
            }
        }
        "ARRAY" => {
            let array_obj = env
                .call_method(
                    obj,
                    "getAsArray",
                    "()[Lorg/finos/legend/pure/rust/PureRustResult;",
                    &[],
                )?
                .l()?;
            let array: jni::objects::JObjectArray = array_obj.into();
            let mut vec = Vec::new();
            let len = env.get_array_length(&array)?;
            for i in 0..len {
                let item = env.get_object_array_element(&array, i)?;
                vec.push(java_to_rust_value(env, &item, context_ptr)?);
            }
            Ok(Value::Collection(Box::new(im_rc::Vector::from_iter(vec))))
        }
        "NULL" => Ok(Value::Unit),
        _ => Err(jni::errors::Error::JavaException),
    }
}

/// Convert a Java RustResult array into a vector of Rust Values
pub fn java_to_rust_args<'local>(
    env: &mut JNIEnv<'local>,
    args_array: &jni::objects::JObjectArray<'local>,
    context_ptr: i64,
) -> Result<Vec<Value>, jni::errors::Error> {
    if args_array.is_null() {
        return Ok(vec![]);
    }

    let mut vec = Vec::new();
    let len = env.get_array_length(args_array)?;
    for i in 0..len {
        let item = env.get_object_array_element(args_array, i)?;
        vec.push(java_to_rust_value(env, &item, context_ptr)?);
    }
    Ok(vec)
}
