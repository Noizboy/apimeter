use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CreditsEnvelope {
  pub data: CreditsData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreditsData {
  pub total_credits: f64,
  pub total_usage: f64,
}

#[derive(Debug, Deserialize)]
pub struct ActivityEnvelope {
  pub data: Vec<ActivityItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivityItem {
  pub model: String,
  pub usage: f64,
  pub date: String,
}
