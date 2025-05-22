# lease2fip-hcloud
Simple controller that updates Hetzner HCloud floating IP assignments to reflect Cilium LB leases.

## Deployment

The application expects a Secret with the name `lease2fip-hcloud` in the same namespace.
There should be a key named `token` which contains an API key to the Hetzner Cloud project your cluster is running in.

You'll also need to configure the whitelist for leases to watch.
You can see an example in [kustomize/example](kustomize/example).

## License

This project is licensed under the terms of the Apache license 2.0.
