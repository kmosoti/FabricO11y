# Local Unix HTTP adapter

Expose the existing typed Reader and Writer operations through an optional
HTTP/1 transport bound to an owned private Unix socket. Keep the launch role
and credential file outside request data, reject browser Origin headers, and
use peer UID as a local operating-system boundary rather than a human identity.

Cap connections, framed input, output and the whole connection lifetime. Keep
the response permit through socket cleanup and canonical query work charged
until its blocking exchange exits. Retry an interrupted acceptance with the same
command ID and body; the response may have been lost after commit. The feature
does not provide TCP, browser authentication, streaming or artifact bytes.
