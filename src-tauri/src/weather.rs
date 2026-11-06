//! 날씨: Open-Meteo (무료, 키 필요 없음). 화면(ui/index.html)의 웹 버전과 같은 JSON 모양으로 돌려준다.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub fn wmo(code: i64) -> &'static str {
    match code {
        0 => "맑음",
        1 => "대체로 맑음",
        2 => "구름 조금",
        3 => "흐림",
        45 => "안개",
        48 => "짙은 안개",
        51 => "약한 이슬비",
        53 => "이슬비",
        55 => "강한 이슬비",
        56 | 57 => "어는 이슬비",
        61 => "약한 비",
        63 => "비",
        65 => "강한 비",
        66 | 67 => "어는 비",
        71 => "약한 눈",
        73 => "눈",
        75 => "많은 눈",
        77 => "싸락눈",
        80 | 81 => "소나기",
        82 => "강한 소나기",
        85 => "눈 소나기",
        86 => "강한 눈 소나기",
        95 => "뇌우",
        96 => "우박 동반 뇌우",
        99 => "강한 우박 뇌우",
        _ => "알 수 없음",
    }
}

/// "대구광역시" → "대구", "서울 강남구" → "강남구"도 시도한다
fn candidates(name: &str) -> Vec<String> {
    let name = name.trim();
    let mut out = vec![name.to_string()];
    for suf in [
        "특별자치시",
        "특별자치도",
        "특별시",
        "광역시",
        "시",
        "군",
        "구",
        "도",
        "동",
        "읍",
        "면",
    ] {
        if let Some(s) = name.strip_suffix(suf) {
            if !s.trim().is_empty() {
                out.push(s.trim().to_string());
            }
            break;
        }
    }
    if let Some(last) = name.split_whitespace().last() {
        out.push(last.to_string());
    }
    out.dedup();
    out
}

async fn geocode(http: &reqwest::Client, name: &str) -> Result<(String, f64, f64)> {
    for q in candidates(name) {
        let v: Value = http
            .get("https://geocoding-api.open-meteo.com/v1/search")
            .query(&[
                ("name", q.as_str()),
                ("count", "10"),
                ("language", "ko"),
                ("format", "json"),
            ])
            .send()
            .await
            .context("날씨 서버에 연결하지 못했습니다")?
            .json()
            .await
            .unwrap_or_default();
        let rs = v["results"].as_array().cloned().unwrap_or_default();
        if rs.is_empty() {
            continue;
        }
        let hangul = q.chars().any(|c| ('가'..='힣').contains(&c));
        let kr: Vec<&Value> = rs
            .iter()
            .filter(|r| hangul && r["country_code"] == "KR")
            .collect();
        let pool: Vec<&Value> = if kr.is_empty() {
            rs.iter().collect()
        } else {
            kr
        };
        let best = pool
            .into_iter()
            .max_by_key(|r| r["population"].as_i64().unwrap_or(0))
            .context("지역을 찾지 못했습니다")?;
        let n = best["name"].as_str().unwrap_or(&q).to_string();
        let shown = match best["admin1"].as_str() {
            Some(a) if !a.starts_with(n.as_str()) => format!("{a} {n}"),
            _ => n,
        };
        return Ok((
            shown,
            best["latitude"].as_f64().unwrap_or_default(),
            best["longitude"].as_f64().unwrap_or_default(),
        ));
    }
    bail!("'{name}' 지역을 찾지 못했습니다. 시나 구 이름으로 다시 말씀해 주세요")
}

pub async fn get(http: &reqwest::Client, place: &str) -> Result<Value> {
    let (name, lat, lon) = geocode(http, place).await?;
    let r = http
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("current", "temperature_2m,apparent_temperature,relative_humidity_2m,precipitation,weather_code,wind_speed_10m".into()),
            ("hourly", "precipitation_probability".into()),
            ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max".into()),
            ("timezone", "auto".into()),
            ("forecast_days", "4".into()),
            ("wind_speed_unit", "ms".into()),
        ])
        .send()
        .await
        .context("날씨 서버에 연결하지 못했습니다")?;
    let ok = r.status().is_success();
    let v: Value = r.json().await.unwrap_or_default();
    if !ok {
        bail!(
            "날씨를 가져오지 못했습니다: {}",
            v["reason"].as_str().unwrap_or("서버 오류")
        );
    }
    Ok(shape(&name, &v))
}

fn round(v: &Value) -> Value {
    v.as_f64()
        .map(|x| json!(x.round() as i64))
        .unwrap_or(Value::Null)
}

pub fn shape(place: &str, v: &Value) -> Value {
    let c = &v["current"];
    let d = &v["daily"];
    let h = &v["hourly"];
    let code = |x: &Value| x.as_i64().unwrap_or(-1);
    let empty = vec![];
    let times = h["time"].as_array().unwrap_or(&empty);
    let now13: String = c["time"].as_str().unwrap_or("").chars().take(13).collect();
    let idx = times
        .iter()
        .position(|t| t.as_str().unwrap_or("") >= now13.as_str())
        .unwrap_or(0);
    let probs = h["precipitation_probability"].as_array().unwrap_or(&empty);
    let next: Vec<(usize, i64)> = probs
        .iter()
        .enumerate()
        .skip(idx)
        .take(12)
        .filter_map(|(i, p)| p.as_i64().map(|p| (i, p)))
        .collect();
    let rain = next
        .iter()
        .max_by_key(|(i, p)| (*p, -(*i as i64)))
        .map(|(i, p)| {
            let at: String = times
                .get(*i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .skip(11)
                .take(5)
                .collect();
            json!({ "maxProb": p, "at": at })
        });
    let days: Vec<Value> = d["time"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let wc = code(&d["weather_code"][i]);
            json!({
                "date": t, "code": wc, "desc": wmo(wc),
                "max": round(&d["temperature_2m_max"][i]), "min": round(&d["temperature_2m_min"][i]),
                "rain": d["precipitation_probability_max"][i],
            })
        })
        .collect();
    let wc = code(&c["weather_code"]);
    json!({
        "place": place,
        "now": {
            "temp": round(&c["temperature_2m"]), "feels": round(&c["apparent_temperature"]),
            "humidity": c["relative_humidity_2m"], "wind": c["wind_speed_10m"], "precip": c["precipitation"],
            "code": wc, "desc": wmo(wc),
        },
        "days": days,
        "rainNext12h": rain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes() {
        assert_eq!(candidates("대구광역시"), vec!["대구광역시", "대구"]);
        assert_eq!(
            candidates("서울 강남구"),
            vec!["서울 강남구", "서울 강남", "강남구"]
        );
    }

    #[test]
    fn shapes_forecast() {
        let v = json!({
            "current": {"time":"2026-11-06T14:00","temperature_2m":12.6,"apparent_temperature":10.2,"relative_humidity_2m":55,"precipitation":0,"weather_code":2,"wind_speed_10m":3.1},
            "hourly": {"time":["2026-11-06T13:00","2026-11-06T14:00","2026-11-06T15:00"],"precipitation_probability":[90,10,40]},
            "daily": {"time":["2026-11-06"],"weather_code":[61],"temperature_2m_max":[15.4],"temperature_2m_min":[6.5],"precipitation_probability_max":[40]}
        });
        let w = shape("대구", &v);
        assert_eq!(w["now"]["temp"], 13);
        assert_eq!(w["now"]["desc"], "구름 조금");
        assert_eq!(w["days"][0]["min"], 7);
        assert_eq!(w["rainNext12h"]["maxProb"], 40);
        assert_eq!(w["rainNext12h"]["at"], "15:00");
    }
}
