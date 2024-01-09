use url::Url;

/// An authorized address combines a URL with an optional bearer token.
#[derive(Clone, Debug)]
pub struct AuthorizedAddress {
    pub url: Url,
    pub bearer_token: Option<String>,
}

impl AuthorizedAddress {
    pub fn join(&self, path: &str) -> AuthorizedAddress {
        let url = self.url.clone();
        let url = url.join(path).expect("URL is always valid");

        Self {
            url,
            bearer_token: self.bearer_token.clone(),
        }
    }

    pub fn to_websocket_address(mut self) -> AuthorizedAddress {
        if self.url.scheme() == "http" {
            self.url
                .set_scheme("ws")
                .expect("should always be able to set URL scheme to static value ws");
        } else if self.url.scheme() == "https" {
            self.url
                .set_scheme("wss")
                .expect("should always be able to set URL scheme to static value wss");
        }

        self
    }

    pub fn bearer_header(&self) -> Option<String> {
        self.bearer_token
            .as_ref()
            .map(|token| format!("Bearer {}", token))
    }
}

impl From<Url> for AuthorizedAddress {
    fn from(url: Url) -> Self {
        let bearer_token = match url.username() {
            "" => None,
            username => Some(username.to_string()),
        };

        let mut url = url;
        url.set_username("").expect("URL is always valid");

        Self { url, bearer_token }
    }
}
