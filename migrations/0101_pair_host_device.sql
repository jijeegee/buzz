-- Group an external agent's computer under the signed-in device it runs on
-- when the keys differ (on a Mac the CLI has its own device key): a pairing
-- the app runs on its own computer says so, and the claim links the external
-- device to the device that started the pairing. Display only; the key match
-- of 0090 still wins. A catalog change only: nullable or constant default,
-- FK NOT VALID, no table rewrite or scan.
SET LOCAL lock_timeout = '1s';

ALTER TABLE pairing_requests
    ADD COLUMN same_computer BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE devices
    ADD COLUMN host_device_id UUID;
ALTER TABLE devices
    ADD CONSTRAINT devices_host_device_id_fkey
        FOREIGN KEY (host_device_id) REFERENCES devices(id) ON DELETE SET NULL NOT VALID;
