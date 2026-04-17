use std::os::fd::RawFd;

use ashpd::desktop::screencast::{
    CursorMode, PersistMode, Screencast, SourceType, Stream, Streams,
};
use ashpd::desktop::{ResponseError, Session};
use ashpd::{Error as AshpdError, WindowIdentifier};
use enumflags2::BitFlags;

pub struct ScreenCastSession {
    _proxy: Screencast<'static>,
    session: Session<'static>,
    pub streams: Vec<Stream>,
    pub pipewire_fd: RawFd,
    pub restore_token: Option<String>,
}

#[allow(dead_code)]
pub enum OpenOutcome {
    Opened(ScreenCastSession),
    Cancelled,
}

impl ScreenCastSession {
    pub async fn open(
        parent: WindowIdentifier,
        restore_token: Option<String>,
    ) -> ashpd::Result<Self> {
        let proxy = Screencast::new().await?;
        let session = proxy.create_session().await?;

        let types: BitFlags<SourceType> = SourceType::Monitor | SourceType::Window;
        let select_req = proxy
            .select_sources(
                &session,
                CursorMode::Embedded,
                types,
                false,
                restore_token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?;
        select_req.response()?;

        let start_req = proxy.start(&session, &parent).await?;
        let streams_response: Streams = start_req.response()?;
        let streams: Vec<Stream> = streams_response.streams().to_vec();
        let restore_token = streams_response.restore_token().map(|s| s.to_owned());

        let pipewire_fd = proxy.open_pipe_wire_remote(&session).await?;

        Ok(Self {
            _proxy: proxy,
            session,
            streams,
            pipewire_fd,
            restore_token,
        })
    }

    pub fn primary_node_id(&self) -> Option<u32> {
        self.streams.first().map(|s| s.pipe_wire_node_id())
    }

    pub fn primary_size(&self) -> Option<(i32, i32)> {
        self.streams.first().and_then(|s| s.size())
    }

    pub async fn close(self) -> ashpd::Result<()> {
        self.session.close().await
    }
}

pub async fn open_or_cancel(
    parent: WindowIdentifier,
    restore_token: Option<String>,
) -> ashpd::Result<OpenOutcome> {
    match ScreenCastSession::open(parent, restore_token).await {
        Ok(session) => Ok(OpenOutcome::Opened(session)),
        Err(AshpdError::Response(ResponseError::Cancelled)) => Ok(OpenOutcome::Cancelled),
        Err(e) => Err(e),
    }
}
