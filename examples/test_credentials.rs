use g360_db_ventas::browser::http::CipsaHttp;
use g360_db_ventas::config::{effective_intranet_user, effective_intranet_pass, load_dotenv};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_dotenv();
    let user = effective_intranet_user();
    let pass = effective_intranet_pass();
    println!("usuario configurado: '{}'", user);
    println!("password configurado: {} chars", pass.len());
    println!();

    // Test 1: login con credenciales reales
    println!("--- Test 1: login con credenciales reales ---");
    match CipsaHttp::new()?.login().await {
        Ok(_) => println!("OK: login exitoso con credenciales actuales"),
        Err(e) => println!("FAIL: {}", e),
    }

    // Test 2: login con password incorrecto
    println!("\n--- Test 2: login con password INCORRECTO ---");
    // Crear un CipsaHttp temporal para forzar password incorrecto
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let login_url = "http://intranet.cipsa.com.pe/intranetcipsa/login.aspx";
    let html = client.get(login_url).send().await?.text().await?;
    // extraer hidden fields
    let mut form: Vec<(String, String)> = Vec::new();
    for m in html.match_indices("<input") {
        let rest = &html[m.0..(m.0 + html[m.0..].len().min(200_000))];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag.find("name=\"").and_then(|i| {
            let s = &tag[i + 6..];
            s.find('"').map(|j| &s[..j])
        }).unwrap_or("");
        if !name.starts_with("__") { continue; }
        let val = tag.find("value=\"").and_then(|i| {
            let s = &tag[i + 7..];
            s.find('"').map(|j| &s[..j])
        }).unwrap_or("");
        form.push((name.to_string(), val.to_string()));
    }
    form.push(("txtnombre".into(), user.clone()));
    form.push(("txtpass".into(), "PASSWORD_INCORRECTO_XYZ".into()));
    form.push(("Button1".into(), "Aceptar".into()));
    let resp = client.post(login_url).form(&form).timeout(std::time::Duration::from_secs(30)).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    let still_on_login = body.contains("txtnombre") && body.contains("txtpass");
    println!("status: {}", status.as_u16());
    println!("sigue en login (rechazado): {}", still_on_login);
    if still_on_login {
        println!("OK: login rechaza credenciales invalidas correctamente");
    } else {
        println!("WARN: login acepto credenciales invalidas");
    }

    // Test 3: verificar que la sesion HTTP funciona (download de 1 dia)
    println!("\n--- Test 3: descarga HTTP de 1 dia para verificar sesion ---");
    let http = CipsaHttp::new()?;
    http.login().await?;
    let range = g360_db_ventas::browser::captor::MonthRange {
        start: chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
        end: chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
        label: "2026-01-15".into(),
    };
    let raw = g360_db_ventas::config::raw_dir();
    std::fs::create_dir_all(&raw)?;
    match tokio::time::timeout(std::time::Duration::from_secs(60), http.download(&range, &raw)).await {
        Ok(Ok(p)) => {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            println!("OK: {} bytes descargados -> {}", size, p.display());
        }
        Ok(Err(e)) => println!("FAIL: {e:#}"),
        Err(_) => println!("TIMEOUT"),
    }
    Ok(())
}
