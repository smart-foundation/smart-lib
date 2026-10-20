use crate::core::Result;
use super::backend::PowerBackend;
use super::types::BatteryState;

pub struct Power<'a, B: PowerBackend + ?Sized>(&'a B);

impl<'a, B: PowerBackend + ?Sized> Power<'a, B> {
    pub fn new(backend: &'a B) -> Self { Power(backend) }
    pub fn battery(&self) -> Result<BatteryState> { self.0.get_battery() }
}
