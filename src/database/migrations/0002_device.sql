-- device.sqlite migration 2: Sign in with ChatGPT.

-- This computer's registration with OpenAI, kept across sign-ins: the host ID
-- names this computer to OpenAI ('urn:uuid:…'), and the client ID is what
-- OpenAI issued Kron on its first sign-in here ('oaiapp_…'). Neither is a
-- secret; the tokens are in the keyring.
ALTER TABLE device_settings ADD COLUMN chatgpt_host_id TEXT;
ALTER TABLE device_settings ADD COLUMN chatgpt_client_id TEXT;
