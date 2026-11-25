//! Component loader for dynamic loading/unloading

use std::future::Future;
use std::pin::Pin;
use crate::{Error, Result};
use orchestrator_common::MetricsRecorder;

/// Trait for loadable components
#[async_trait::async_trait]
pub trait Loadable: Send + Sync {
    type Config;
    
    async fn load(config: &Self::Config) -> Result<Self> where Self: Sized;
    async fn shutdown(&mut self);
    fn name(&self) -> &str;
}

/// Component loader with lifecycle management
pub struct ComponentLoader<T: Loadable> {
    component: Option<T>,
    config: Option<T::Config>,
    metrics: MetricsRecorder,
}

impl<T: Loadable> ComponentLoader<T> {
    pub fn new(config: T::Config) -> Self {
        Self {
            component: None,
            config: Some(config),
            metrics: MetricsRecorder::new(&format!("loader.{}", std::any::type_name::<T>()))
        }
    }

    pub async fn ensure_loaded(&mut self) -> Result<&T> {
        if self.component.is_none() {
            let config = self.config.take().ok_or_else(|| Error::InvalidState("Config already consumed".to_string()))?;
            let component = T::load(&config).await?;
            self.metrics.counter("load", 1);
            self.component = Some(component);
        }
        Ok(self.component.as_ref().unwrap())
    }

    pub async fn unload(&mut self) {
        if let Some(mut component) = self.component.take() {
            component.shutdown().await;
            self.metrics.counter("unload", 1);
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.component.is_some()
    }

    pub fn get(&self) -> Option<&T> {
        self.component.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.component.as_mut()
    }
}

/// Loader for components that need async initialization with event bus
pub struct AsyncComponentLoader<T, F, Fut> 
where
    F: FnOnce(T::Config) -> Fut + Send,
    Fut: Future<Output = Result<T>> + Send,
    T: Loadable + Send,
{
    component: Option<T>,
    config: Option<T::Config>,
    init_fn: Option<F>,
    metrics: MetricsRecorder,
}

impl<T, F, Fut> AsyncComponentLoader<T, F, Fut>
where
    F: FnOnce(T::Config) -> Fut + Send,
    Fut: Future<Output = Result<T>> + Send,
    T: Loadable + Send,
{
    pub fn new(config: T::Config, init_fn: F) -> Self {
        Self {
            component: None,
            config: Some(config),
            init_fn: Some(init_fn),
            metrics: MetricsRecorder::new(&format!("async_loader.{}", std::any::type_name::<T>())),
        }
    }

    pub async fn ensure_loaded(&mut self) -> Result<&T> {
        if self.component.is_none() {
            let config = self.config.take().ok_or_else(|| Error::InvalidState("Config already consumed".to_string()))?;
            let init_fn = self.init_fn.take().ok_or_else(|| Error::InvalidState("Init fn already consumed".to_string()))?;
            let component = init_fn(config).await?;
            self.metrics.counter("load", 1);
            self.component = Some(component);
        }
        Ok(self.component.as_ref().unwrap())
    }

    pub async fn unload(&mut self) {
        if let Some(mut component) = self.component.take() {
            component.shutdown().await;
            self.metrics.counter("unload", 1);
        }
    }
}