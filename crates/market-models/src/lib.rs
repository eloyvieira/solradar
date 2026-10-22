pub fn buy_count_ratio(buys: u32, sells: u32) -> Option<f64> {
    let n = u64::from(buys) + u64::from(sells);
    if n == 0 { None } else { Some(f64::from(buys) / n as f64) }
}
pub fn volume_acceleration(current: f64, previous: f64, age_seconds: i64) -> Option<f64> {
    if !(300..=420).contains(&age_seconds) || !current.is_finite() || !previous.is_finite() || previous < 100.0 { None }
    else { Some(current / previous) }
}
pub fn spread_percent(a: f64, b: f64) -> Option<f64> {
    if a <= 0.0 || b <= 0.0 || !a.is_finite() || !b.is_finite() { None }
    else { Some((a.max(b)/a.min(b)-1.0)*100.0) }
}
pub fn trailing_stop(peak: f64, current: f64, previous_stop: f64, trailing_pct: f64) -> Option<(f64,f64,bool)> {
    if ![peak,current,previous_stop,trailing_pct].iter().all(|x| x.is_finite()) || current <= 0.0 || !(0.0..100.0).contains(&trailing_pct) {return None}
    let new_peak=peak.max(current);
    let stop=previous_stop.max(new_peak*(1.0-trailing_pct/100.0));
    Some((new_peak,stop,current <= stop))
}
#[cfg(test)] mod tests {
  use super::*;
  #[test] fn ratio(){assert_eq!(buy_count_ratio(0,0),None);assert_eq!(buy_count_ratio(3,1),Some(0.75));}
  #[test] fn volume(){assert_eq!(volume_acceleration(1000.0,500.0,300),Some(2.0));assert!(volume_acceleration(1000.0,0.0,300).is_none());}
  #[test] fn spread(){assert!((spread_percent(10.0,11.0).unwrap()-10.0).abs()<1e-9);}
  #[test] fn stop(){let (peak,stop,exit)=trailing_stop(100.0,110.0,95.0,5.0).unwrap();assert_eq!(peak,110.0);assert!((stop-104.5).abs()<1e-9);assert!(!exit);}
}
