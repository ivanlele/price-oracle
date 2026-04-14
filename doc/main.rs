use axum::Router;
use tokio::net::TcpListener;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use price_oracle::handlers::ApiDoc;

#[tokio::main]
async fn main() {
    let openapi = ApiDoc::openapi();

    let app =
        Router::new().merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", openapi));

    let listener = TcpListener::bind("0.0.0.0:8081").await.unwrap();
    eprintln!("Swagger UI available at http://localhost:8081/swagger-ui/");
    axum::serve(listener, app).await.unwrap();
}
