use std::sync::Arc;

use ahash::AHashMap;
use my_http_client::MyHttpClientConnector;

use parking_lot::Mutex;
use rust_extensions::date_time::DateTimeAsMicroseconds;
use tokio::net::TcpStream;

#[cfg(feature = "_tls")]
use my_tls::tokio_rustls::client::TlsStream;

use crate::{non_wasm::http_connectors::*, non_wasm::my_http_client_wrapper::MyHttpClientWrapper, ConnectionParams};

pub struct ConnectionItem<
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
    TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
> {
    pub last_update: DateTimeAsMicroseconds,
    pub connection: Arc<MyHttpClientWrapper<TStream, TConnector>>,
}

pub struct FlUrlHttpConnectionsCacheInner {
    max_connections: usize,
    http: AHashMap<String, Vec<ConnectionItem<TcpStream, HttpConnector>>>,
    #[cfg(feature = "_tls")]
    https: AHashMap<String, Vec<ConnectionItem<TlsStream<TcpStream>, HttpsConnector>>>,
    #[cfg(unix)]
    unix_socket: AHashMap<String, Vec<ConnectionItem<UnixSocketStream, UnixSocketConnector>>>,
    #[cfg(all(unix, feature = "with-ssh"))]
    ssh: AHashMap<String, Vec<ConnectionItem<my_ssh::SshAsyncChannel, SshHttpConnector>>>,
    #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
    ssh_https: AHashMap<
        String,
        Vec<ConnectionItem<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>>,
    >,
}

impl Default for FlUrlHttpConnectionsCacheInner {
    fn default() -> Self {
        Self {
            max_connections: 5,
            http: Default::default(),
            #[cfg(feature = "_tls")]
            https: Default::default(),
            #[cfg(unix)]
            unix_socket: Default::default(),
            #[cfg(all(unix, feature = "with-ssh"))]
            ssh: Default::default(),
            #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
            ssh_https: Default::default(),
        }
    }
}

pub struct FlUrlHttpConnectionsCache {
    pub inner: Mutex<FlUrlHttpConnectionsCacheInner>,
}

impl FlUrlHttpConnectionsCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FlUrlHttpConnectionsCacheInner::default()),
        }
    }

    pub fn new_with_max_connections(max_connections: usize) -> Self {
        let inner = FlUrlHttpConnectionsCacheInner {
            max_connections,
            ..Default::default()
        };
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// Drops every pooled connection. Existing checked-out connections are
    /// unaffected and get disposed when their last user drops them.
    pub fn clear(&self) {
        let mut write_access = self.inner.lock();
        write_access.http.clear();
        #[cfg(feature = "_tls")]
        write_access.https.clear();
        #[cfg(unix)]
        write_access.unix_socket.clear();
        #[cfg(all(unix, feature = "with-ssh"))]
        write_access.ssh.clear();
        #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
        write_access.ssh_https.clear();
    }

    /// Removes idle connections that outlived `reuse_connection_timeout` and
    /// empty per-host slots, so the process-global cache does not grow without
    /// bound for hosts that are never contacted again.
    pub fn gc(&self, reuse_connection_timeout_seconds: i64) {
        let now = DateTimeAsMicroseconds::now();
        let mut write_access = self.inner.lock();
        gc_map(&mut write_access.http, now, reuse_connection_timeout_seconds);
        #[cfg(feature = "_tls")]
        gc_map(
            &mut write_access.https,
            now,
            reuse_connection_timeout_seconds,
        );
        #[cfg(unix)]
        gc_map(
            &mut write_access.unix_socket,
            now,
            reuse_connection_timeout_seconds,
        );
        #[cfg(all(unix, feature = "with-ssh"))]
        gc_map(&mut write_access.ssh, now, reuse_connection_timeout_seconds);
        #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
        gc_map(
            &mut write_access.ssh_https,
            now,
            reuse_connection_timeout_seconds,
        );
    }

    pub async fn get_http_connection(
        &self,
        params: &ConnectionParams<'_>,
    ) -> Arc<MyHttpClientWrapper<TcpStream, HttpConnector>> {
        let connection_key = super::utils::get_http_connection_key(params);

        let mut write_access = self.inner.lock();

        checkout_connection(
            &mut write_access.http,
            connection_key.as_str(),
            params.reuse_connection_timeout_seconds,
            params.mode.is_h2(),
            || {
                super::creators::HttpConnectionCreator::create_connection(
                    params,
                    connection_key.to_string(),
                )
            },
        )
    }

    pub fn put_http_connection_back_sync(
        &self,
        connection: Arc<MyHttpClientWrapper<TcpStream, HttpConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        let max_connections = write_access.max_connections;
        put_connection_back(&mut write_access.http, max_connections, connection);
    }

    pub async fn put_http_connection_back(
        &self,
        connection: Arc<MyHttpClientWrapper<TcpStream, HttpConnector>>,
    ) {
        self.put_http_connection_back_sync(connection);
    }

    pub fn drop_http_connection_sync(
        &self,
        connection: &Arc<MyHttpClientWrapper<TcpStream, HttpConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        remove_connection(&mut write_access.http, connection);
    }

    #[cfg(feature = "_tls")]
    pub async fn get_https_connection(
        &self,
        params: &ConnectionParams<'_>,
    ) -> Arc<MyHttpClientWrapper<TlsStream<TcpStream>, HttpsConnector>> {
        let connection_key = super::utils::get_https_connection_key(params);

        let mut write_access = self.inner.lock();

        checkout_connection(
            &mut write_access.https,
            connection_key.as_str(),
            params.reuse_connection_timeout_seconds,
            params.mode.is_h2(),
            || {
                super::creators::HttpsConnectionCreator::create_connection(
                    params,
                    connection_key.to_string(),
                )
            },
        )
    }

    #[cfg(feature = "_tls")]
    pub fn put_https_connection_back_sync(
        &self,
        connection: Arc<MyHttpClientWrapper<TlsStream<TcpStream>, HttpsConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        let max_connections = write_access.max_connections;
        put_connection_back(&mut write_access.https, max_connections, connection);
    }

    #[cfg(feature = "_tls")]
    pub async fn put_https_connection_back(
        &self,
        connection: Arc<MyHttpClientWrapper<TlsStream<TcpStream>, HttpsConnector>>,
    ) {
        self.put_https_connection_back_sync(connection);
    }

    #[cfg(feature = "_tls")]
    pub fn drop_https_connection_sync(
        &self,
        connection: &Arc<MyHttpClientWrapper<TlsStream<TcpStream>, HttpsConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        remove_connection(&mut write_access.https, connection);
    }

    /// `Err` when `params` carry no ssh session: there is nowhere to open the tunnel.
    #[cfg(all(unix, feature = "with-ssh"))]
    pub async fn get_ssh_connection(
        &self,
        params: &ConnectionParams<'_>,
    ) -> Result<
        Arc<MyHttpClientWrapper<my_ssh::SshAsyncChannel, SshHttpConnector>>,
        my_http_client::MyHttpClientError,
    > {
        let Some(ssh_session) = params.ssh_session.clone() else {
            return Err(super::creators::no_ssh_session());
        };

        let connection_key = super::utils::get_ssh_connection_key(
            ssh_session.get_ssh_credentials(),
            params.remote_endpoint,
            params.resolved_ip,
            params.mode,
        );

        let mut write_access = self.inner.lock();

        Ok(checkout_connection(
            &mut write_access.ssh,
            connection_key.as_str(),
            params.reuse_connection_timeout_seconds,
            params.mode.is_h2(),
            || {
                super::creators::SshConnectionCreator::create_connection(
                    params,
                    ssh_session.clone(),
                    connection_key.to_string(),
                )
            },
        ))
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn put_ssh_connection_back_sync(
        &self,
        connection: Arc<MyHttpClientWrapper<my_ssh::SshAsyncChannel, SshHttpConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        let max_connections = write_access.max_connections;
        put_connection_back(&mut write_access.ssh, max_connections, connection);
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub async fn put_ssh_connection_back(
        &self,
        connection: Arc<MyHttpClientWrapper<my_ssh::SshAsyncChannel, SshHttpConnector>>,
    ) {
        self.put_ssh_connection_back_sync(connection);
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn drop_ssh_connection_sync(
        &self,
        connection: &Arc<MyHttpClientWrapper<my_ssh::SshAsyncChannel, SshHttpConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        remove_connection(&mut write_access.ssh, connection);
    }

    /// `Err` when `params` carry no ssh session: there is nowhere to open the tunnel.
    #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
    pub async fn get_ssh_https_connection(
        &self,
        params: &ConnectionParams<'_>,
    ) -> Result<
        Arc<MyHttpClientWrapper<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>>,
        my_http_client::MyHttpClientError,
    > {
        let Some(ssh_session) = params.ssh_session.clone() else {
            return Err(super::creators::no_ssh_session());
        };

        let connection_key =
            super::utils::get_ssh_https_connection_key(ssh_session.get_ssh_credentials(), params);

        let mut write_access = self.inner.lock();

        Ok(checkout_connection(
            &mut write_access.ssh_https,
            connection_key.as_str(),
            params.reuse_connection_timeout_seconds,
            params.mode.is_h2(),
            || {
                super::creators::SshHttpsConnectionCreator::create_connection(
                    params,
                    ssh_session.clone(),
                    connection_key.to_string(),
                )
            },
        ))
    }

    #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
    pub fn put_ssh_https_connection_back_sync(
        &self,
        connection: Arc<MyHttpClientWrapper<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        let max_connections = write_access.max_connections;
        put_connection_back(&mut write_access.ssh_https, max_connections, connection);
    }

    #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
    pub async fn put_ssh_https_connection_back(
        &self,
        connection: Arc<MyHttpClientWrapper<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>>,
    ) {
        self.put_ssh_https_connection_back_sync(connection);
    }

    #[cfg(all(unix, feature = "with-ssh", feature = "_tls"))]
    pub fn drop_ssh_https_connection_sync(
        &self,
        connection: &Arc<MyHttpClientWrapper<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        remove_connection(&mut write_access.ssh_https, connection);
    }

    #[cfg(unix)]
    pub async fn get_unix_socket_connection(
        &self,
        params: &ConnectionParams<'_>,
    ) -> Arc<MyHttpClientWrapper<UnixSocketStream, UnixSocketConnector>> {
        let connection_key = super::utils::get_unix_socket_connection_key(params);

        let mut write_access = self.inner.lock();

        checkout_connection(
            &mut write_access.unix_socket,
            connection_key.as_str(),
            params.reuse_connection_timeout_seconds,
            params.mode.is_h2(),
            || {
                super::creators::UnixSocketHttpClientCreator::create_connection(
                    params,
                    connection_key.to_string(),
                )
            },
        )
    }

    #[cfg(unix)]
    pub fn put_unix_socket_connection_back_sync(
        &self,
        connection: Arc<MyHttpClientWrapper<UnixSocketStream, UnixSocketConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        let max_connections = write_access.max_connections;
        put_connection_back(&mut write_access.unix_socket, max_connections, connection);
    }

    #[cfg(unix)]
    pub async fn put_unix_socket_connection_back(
        &self,
        connection: Arc<MyHttpClientWrapper<UnixSocketStream, UnixSocketConnector>>,
    ) {
        self.put_unix_socket_connection_back_sync(connection);
    }

    #[cfg(unix)]
    pub fn drop_unix_socket_connection_sync(
        &self,
        connection: &Arc<MyHttpClientWrapper<UnixSocketStream, UnixSocketConnector>>,
    ) {
        let mut write_access = self.inner.lock();
        remove_connection(&mut write_access.unix_socket, connection);
    }
}

impl Default for FlUrlHttpConnectionsCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Pool contract: HTTP/1 connections are checked out EXCLUSIVELY — the item is
/// removed from the pool for the duration of the request (and of the response
/// body) and comes back via `put_connection_back` only when it is safe to reuse.
/// H2 connections multiplex, so a single client per key is SHARED: checkout
/// clones the Arc and leaves the item in place.
fn checkout_connection<
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
    TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
>(
    connections: &mut AHashMap<String, Vec<ConnectionItem<TStream, TConnector>>>,
    hash_map_key: &str,
    connection_timeout_seconds: i64,
    is_h2: bool,
    create_connection: impl Fn() -> Arc<MyHttpClientWrapper<TStream, TConnector>>,
) -> Arc<MyHttpClientWrapper<TStream, TConnector>> {
    let now = DateTimeAsMicroseconds::now();

    if let Some(items) = connections.get_mut(hash_map_key) {
        // Expired idle connections are dropped (drop of the last Arc disposes them)
        items.retain(|itm| {
            now.duration_since(itm.last_update).get_full_seconds() < connection_timeout_seconds
        });

        if is_h2 {
            if let Some(item) = items.first_mut() {
                item.last_update = now;
                return item.connection.clone();
            }
        } else if let Some(item) = items.pop() {
            if items.is_empty() {
                connections.remove(hash_map_key);
            }
            return item.connection;
        }

        if items.is_empty() {
            connections.remove(hash_map_key);
        }
    }

    let new_one = create_connection();

    if is_h2 {
        connections.insert(
            hash_map_key.to_string(),
            vec![ConnectionItem {
                last_update: now,
                connection: new_one.clone(),
            }],
        );
    }
    // HTTP/1: the new connection is checked out — it enters the pool only via
    // put_connection_back, once the response body has been fully consumed.

    new_one
}

fn put_connection_back<
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
    TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
>(
    connections: &mut AHashMap<String, Vec<ConnectionItem<TStream, TConnector>>>,
    max_connections: usize,
    connection: Arc<MyHttpClientWrapper<TStream, TConnector>>,
) {
    let now = DateTimeAsMicroseconds::now();

    if connection.is_h2() {
        // The shared H2 client is already in the map — just refresh its stamp.
        if let Some(items) = connections.get_mut(&connection.key) {
            for item in items.iter_mut() {
                if Arc::ptr_eq(&item.connection, &connection) {
                    item.last_update = now;
                    return;
                }
            }
        }
        // It was removed (error path or gc) while this clone was in flight —
        // do not resurrect it, a fresh client gets created on the next request.
        return;
    }

    let items = connections.entry(connection.key.to_string()).or_default();

    if items.len() < max_connections {
        items.push(ConnectionItem {
            last_update: now,
            connection,
        });
    }
    // else: pool is full — dropping the Arc disposes the connection.
}

fn remove_connection<
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
    TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
>(
    connections: &mut AHashMap<String, Vec<ConnectionItem<TStream, TConnector>>>,
    connection: &Arc<MyHttpClientWrapper<TStream, TConnector>>,
) {
    if let Some(items) = connections.get_mut(&connection.key) {
        items.retain(|itm| !Arc::ptr_eq(&itm.connection, connection));
        if items.is_empty() {
            connections.remove(&connection.key);
        }
    }
}

fn gc_map<
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
    TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
>(
    connections: &mut AHashMap<String, Vec<ConnectionItem<TStream, TConnector>>>,
    now: DateTimeAsMicroseconds,
    timeout_seconds: i64,
) {
    connections.retain(|_, items| {
        items.retain(|itm| {
            now.duration_since(itm.last_update).get_full_seconds() < timeout_seconds
        });
        !items.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlUrlMode;
    use rust_extensions::remote_endpoint::RemoteEndpointOwned;

    fn make_params(endpoint: &RemoteEndpointOwned, mode: FlUrlMode) -> ConnectionParams<'_> {
        ConnectionParams {
            mode,
            remote_endpoint: endpoint.to_ref(),
            resolved_ip: None,
            host_header: None,
            #[cfg(feature = "_tls")]
            client_certificate: None,
            accept_invalid_certificate: false,
            reuse_connection_timeout_seconds: 120,
            #[cfg(all(unix, feature = "with-ssh"))]
            ssh_session: None,
            #[cfg(all(unix, feature = "with-ssh"))]
            is_unix_socket: false,
        }
    }

    /// The ssh path takes the session out of the params. Without one there is nowhere
    /// to open the tunnel, and that is an error of the request, not a panic.
    #[cfg(all(unix, feature = "with-ssh"))]
    #[tokio::test]
    async fn an_ssh_connection_with_no_session_is_an_error() {
        use super::super::creators::SshConnectionCreator;
        use super::super::HttpConnectionResolver;

        let endpoint = RemoteEndpointOwned::try_parse("http://localhost:9999".to_string()).unwrap();
        let params = make_params(&endpoint, FlUrlMode::Http1Hyper);

        let cache = FlUrlHttpConnectionsCache::new();
        assert!(matches!(
            cache.get_ssh_connection(&params).await,
            Err(my_http_client::MyHttpClientError::CanNotConnectToRemoteHost(_))
        ));

        let resolver: &dyn HttpConnectionResolver<my_ssh::SshAsyncChannel, SshHttpConnector> =
            &SshConnectionCreator;
        assert!(matches!(
            resolver.get_http_connection(&params).await,
            Err(my_http_client::MyHttpClientError::CanNotConnectToRemoteHost(_))
        ));
    }

    #[tokio::test]
    async fn http1_connection_is_checked_out_exclusively() {
        let cache = FlUrlHttpConnectionsCache::new();
        let endpoint = RemoteEndpointOwned::try_parse("http://localhost:9999".to_string()).unwrap();
        let params = make_params(&endpoint, FlUrlMode::Http1Hyper);

        let first = cache.get_http_connection(&params).await;
        let second = cache.get_http_connection(&params).await;

        // While the first connection is checked out, a concurrent request must
        // get a DIFFERENT connection, not share the same HTTP/1 client.
        assert!(!Arc::ptr_eq(&first, &second));

        cache.put_http_connection_back_sync(first.clone());
        let third = cache.get_http_connection(&params).await;
        assert!(Arc::ptr_eq(&first, &third));
    }

    #[tokio::test]
    async fn h2_connection_is_shared() {
        let cache = FlUrlHttpConnectionsCache::new();
        let endpoint = RemoteEndpointOwned::try_parse("http://localhost:9999".to_string()).unwrap();
        let params = make_params(&endpoint, FlUrlMode::H2);

        let first = cache.get_http_connection(&params).await;
        let second = cache.get_http_connection(&params).await;

        // H2 multiplexes: both requests share the same client.
        assert!(Arc::ptr_eq(&first, &second));

        // An error evicts the shared client; the next request gets a fresh one.
        cache.drop_http_connection_sync(&first);
        let third = cache.get_http_connection(&params).await;
        assert!(!Arc::ptr_eq(&first, &third));
    }

    #[tokio::test]
    async fn different_modes_do_not_share_connections() {
        let cache = FlUrlHttpConnectionsCache::new();
        let endpoint = RemoteEndpointOwned::try_parse("http://localhost:9999".to_string()).unwrap();

        let h1 = cache
            .get_http_connection(&make_params(&endpoint, FlUrlMode::Http1Hyper))
            .await;
        cache.put_http_connection_back_sync(h1.clone());

        // A NoHyper request against the same host:port must not receive the
        // pooled Http1Hyper wrapper — that mix used to panic in unwrap_as_*.
        let no_hyper = cache
            .get_http_connection(&make_params(&endpoint, FlUrlMode::Http1NoHyper))
            .await;
        assert!(!Arc::ptr_eq(&h1, &no_hyper));
    }

    #[tokio::test]
    async fn same_host_pinned_to_different_ips_does_not_share_connections() {
        let cache = FlUrlHttpConnectionsCache::new();
        let endpoint = RemoteEndpointOwned::try_parse("http://domain.com:9999".to_string()).unwrap();

        let mut first_ip = make_params(&endpoint, FlUrlMode::Http1Hyper);
        first_ip.resolved_ip = Some("10.0.0.1".parse().unwrap());
        let mut second_ip = make_params(&endpoint, FlUrlMode::Http1Hyper);
        second_ip.resolved_ip = Some("10.0.0.2".parse().unwrap());
        let resolved_by_dns = make_params(&endpoint, FlUrlMode::Http1Hyper);

        let pooled = cache.get_http_connection(&first_ip).await;
        cache.put_http_connection_back_sync(pooled.clone());

        // Same Host, same port — but a socket to 10.0.0.1 is not a socket to
        // 10.0.0.2, nor to whatever DNS says domain.com is.
        let other_ip = cache.get_http_connection(&second_ip).await;
        assert!(!Arc::ptr_eq(&pooled, &other_ip));

        let by_dns = cache.get_http_connection(&resolved_by_dns).await;
        assert!(!Arc::ptr_eq(&pooled, &by_dns));

        let same_ip = cache.get_http_connection(&first_ip).await;
        assert!(Arc::ptr_eq(&pooled, &same_ip));
    }

    #[tokio::test]
    async fn put_back_respects_max_connections() {
        let cache = FlUrlHttpConnectionsCache::new_with_max_connections(2);
        let endpoint = RemoteEndpointOwned::try_parse("http://localhost:9999".to_string()).unwrap();
        let params = make_params(&endpoint, FlUrlMode::Http1Hyper);

        let c1 = cache.get_http_connection(&params).await;
        let c2 = cache.get_http_connection(&params).await;
        let c3 = cache.get_http_connection(&params).await;

        cache.put_http_connection_back_sync(c1.clone());
        cache.put_http_connection_back_sync(c2.clone());
        cache.put_http_connection_back_sync(c3.clone());

        let pooled = cache.inner.lock().http.get(&c1.key).map(|v| v.len());
        assert_eq!(pooled, Some(2));
    }
}
