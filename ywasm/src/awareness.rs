use js_sys::Uint8Array;
use serde::Serialize;
use std::cell::{Ref, RefCell, RefMut};
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

use yrs::protocol::{Awareness as YAwareness, AwarenessUpdate, Timestamp};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::ClientID;

use crate::doc::YDoc;
use crate::js::{Callback, Js};

/// An event yrs raised during a mutation, delivered to its JS callback once the mutation is done.
struct Pending {
    callback: js_sys::Function,
    summary: crate::Result<JsValue>,
    origin: JsValue,
}

/// yrs calls awareness observers synchronously, inside the `&mut` method that changed the state.
/// A JS handler typically reads the awareness again (`getStates()`, `encodeAwarenessUpdate`), so
/// the observers only queue the event, and every mutating method delivers the queue after it has
/// released its borrow of the state.
#[wasm_bindgen]
pub struct Awareness {
    inner: RefCell<YAwareness>,
    // A `Mutex` only because the observers must be `Send` when yrs is built with `sync`.
    pending: Arc<Mutex<Vec<Pending>>>,
}

impl Awareness {
    fn inner(&self) -> crate::Result<Ref<'_, YAwareness>> {
        self.inner
            .try_borrow()
            .map_err(|_| JsValue::from_str("awareness is being modified"))
    }

    fn inner_mut(&self) -> crate::Result<RefMut<'_, YAwareness>> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("awareness is in use"))
    }

    /// Runs `f` on the state, then calls the JS callbacks for the events it raised. Every
    /// callback is called; the first one that throws is reported.
    fn mutate<R>(&self, f: impl FnOnce(&mut YAwareness) -> R) -> crate::Result<R> {
        let result = f(&mut *self.inner_mut()?);
        self.flush()?;
        Ok(result)
    }

    fn flush(&self) -> crate::Result<()> {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        let mut first_err = None;
        for p in pending {
            let res = p
                .summary
                .and_then(|json| p.callback.call2(&JsValue::NULL, &json, &p.origin));
            if let Err(e) = res {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }
}

#[wasm_bindgen]
impl Awareness {
    #[wasm_bindgen(constructor)]
    pub fn new(doc: YDoc) -> Awareness {
        let inner = RefCell::new(YAwareness::with_clock(doc.0.clone(), JsClock));
        Awareness {
            inner,
            pending: Arc::default(),
        }
    }

    #[wasm_bindgen(getter, js_name = doc)]
    pub fn doc(&self) -> crate::Result<YDoc> {
        Ok(YDoc(self.inner()?.doc().clone()))
    }

    #[wasm_bindgen(getter, js_name = meta)]
    pub fn meta(&self) -> crate::Result<js_sys::Map> {
        #[derive(Serialize)]
        struct ClientStateMeta {
            clock: u32,
            last_updated: u64,
        }
        let result = js_sys::Map::new();
        for (client_id, state) in self.inner()?.iter() {
            let info = ClientStateMeta {
                clock: state.clock,
                last_updated: state.last_updated,
            };
            let info = crate::js::to_js(&info).map_err(|e| JsValue::from_str(&e.to_string()))?;
            result.set(&JsValue::from_f64(client_id.get() as f64), &info);
        }
        Ok(result)
    }

    #[wasm_bindgen(js_name = destroy)]
    pub fn destroy(&self) -> crate::Result<()> {
        self.mutate(|inner| inner.clean_local_state())
    }

    #[wasm_bindgen(js_name = getLocalState)]
    pub fn local_state(&self) -> crate::Result<JsValue> {
        let state = self.inner()?.local_state_raw();
        match state {
            None => Ok(JsValue::NULL),
            Some(js) => js_sys::JSON::parse(js.as_ref()),
        }
    }

    #[wasm_bindgen(js_name = setLocalState)]
    pub fn set_local_state(&self, state: JsValue) -> crate::Result<()> {
        if state.is_null() {
            self.mutate(|inner| inner.clean_local_state())
        } else {
            let json = js_sys::JSON::stringify(&state)?.as_string().unwrap();
            self.mutate(|inner| inner.set_local_state_raw(json))
        }
    }

    #[wasm_bindgen(js_name = setLocalStateField)]
    pub fn set_field(&self, key: &str, value: JsValue) -> crate::Result<()> {
        let state = self.local_state()?;
        js_sys::Reflect::set(&state, &JsValue::from_str(key), &value)?;
        self.set_local_state(state)
    }

    #[wasm_bindgen(js_name = getStates)]
    pub fn states(&self) -> crate::Result<js_sys::Map> {
        let result = js_sys::Map::new();
        for (client_id, state) in self.inner()?.iter() {
            if let Some(data) = &state.data {
                let state = js_sys::JSON::parse(data.as_ref())?;
                result.set(&JsValue::from_f64(client_id.get() as f64), &state);
            }
        }
        Ok(result)
    }

    #[wasm_bindgen(js_name = on)]
    pub fn on(&self, event: &str, callback: js_sys::Function) -> crate::Result<()> {
        let abi = callback.subscription_key();
        let pending = self.pending.clone();
        let queue = move |_: &YAwareness,
                          e: &yrs::protocol::awareness::Event,
                          origin: Option<&yrs::Origin>| {
            pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Pending {
                    callback: callback.clone(),
                    summary: crate::js::to_js(e.summary())
                        .map_err(|e| JsValue::from_str(&e.to_string())),
                    origin: match origin {
                        None => JsValue::UNDEFINED,
                        Some(origin) => Js::from(origin).into(),
                    },
                });
        };
        let mut inner = self.inner_mut()?;
        match event {
            "update" => inner.on_update(abi, queue),
            "change" => inner.on_change(abi, queue),
            unknown => return Err(JsValue::from_str(&format!("Unknown event: {}", unknown))),
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = off)]
    pub fn off(&self, event: &str, callback: js_sys::Function) -> crate::Result<bool> {
        let abi = callback.subscription_key();
        let mut inner = self.inner_mut()?;
        match event {
            "update" => Ok(inner.unobserve_update(abi)),
            "change" => Ok(inner.unobserve_change(abi)),
            unknown => return Err(JsValue::from_str(&format!("Unknown event: {}", unknown))),
        }
    }
}

#[wasm_bindgen(js_name = removeAwarenessStates)]
pub fn remove_states(awareness: &Awareness, clients: Vec<u64>) -> crate::Result<()> {
    awareness.mutate(|inner| {
        for client_id in clients {
            inner.remove_state(ClientID::new(client_id));
        }
    })
}

#[wasm_bindgen(js_name = encodeAwarenessUpdate)]
pub fn encode_update(awareness: &Awareness, clients: JsValue) -> crate::Result<Uint8Array> {
    // Read `clients` before borrowing the state: converting it can run JS getters.
    let client_ids: Option<Vec<u64>> = if clients.is_null() || clients.is_undefined() {
        None
    } else {
        Some(
            serde_wasm_bindgen::from_value(clients)
                .map_err(|e| JsValue::from_str(&e.to_string()))?,
        )
    };
    let inner = awareness.inner()?;
    let res = match client_ids {
        None => inner.update(),
        Some(ids) => inner.update_with_clients(ids.into_iter().map(ClientID::new)),
    };

    let update = res.map_err(|e| JsValue::from_str(&e.to_string()))?;
    let bytes = update.encode_v1();
    Ok(Uint8Array::from(bytes.as_slice()))
}

#[wasm_bindgen(js_name = modifyAwarenessUpdate)]
pub fn modify_update(update: Uint8Array, modify: js_sys::Function) -> crate::Result<Uint8Array> {
    let mut update = AwarenessUpdate::decode_v1(&update.to_vec())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    for (client_id, state) in update.clients.iter_mut() {
        let js = js_sys::JSON::parse(&state.json)?;
        let new_state = modify.call2(&JsValue::NULL, &js, &JsValue::from(client_id.get()))?;
        let json: String = js_sys::JSON::stringify(&new_state)?.into();
        state.json = json.into();
    }
    Ok(Uint8Array::from(update.encode_v1().as_slice()))
}

#[wasm_bindgen(js_name = applyAwarenessUpdate)]
pub fn apply_update(
    awareness: &Awareness,
    update: Uint8Array,
    _origin: JsValue, //TODO: use origin in Awareness::apply_update
) -> crate::Result<()> {
    let update = AwarenessUpdate::decode_v1(&update.to_vec())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    awareness
        .mutate(|inner| inner.apply_update(update))?
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

pub struct JsClock;

impl yrs::time::Clock for JsClock {
    fn now(&self) -> Timestamp {
        js_sys::Date::now() as u64
    }
}
