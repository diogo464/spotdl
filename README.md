# spotdl
------
spotdl is a cli tool to archive and synchronize spotify tracks for offline usage. Its main use case is to be able to listen to music on devices that have poor or no support for spotify. spotdl requires a spotify premium account.

## Basic Examples

### Logging in to spotify
```
spotdl login $USERNAME $PASSWORD
```

### Synchronizing an album to a directory
```bash
# It is also possible to use an ID or URL instead of a URI
spotdl download spotify:album:7vfuTRXIAYJz5Uc8SddnTr
```

The download command tries to avoid downloading any already existing tracks by looking at the metadata of existing tracks in the destination directory.

![](media/spotdl-download-album.gif)

### Synchronizing a playlist to a directory
```bash
spotdl download spotify:playlist:7dXMxn9chL56TWB31fa33J
```

By default spotdl will cache playlist metadata for a few hours so if you want to make sure you are synchronizing the latest playlist just remove it from the cache first with:

```bash
spotdl cache remove spotify:playlist:7dXMxn9chL56TWB31fa33J
```

## Commands

### **spotdl login**
Login to a spotify account using username and password.
Only spotify premium accounts are supported.
```bash
spotdl login $USERNAME $PASSWORD
```
After the login succeeds a token is stored in the cache directory, the password itself is not stored on disk.

### **spotdl logout**
Logout from a spotify account.
```bash
spotdl logout $USERNAME
```
This is equivalent to just removing the file storing the credentials.

### **spotdl info**
Print information about a spotify resource.
```bash
spotdl info spotify:playlist:7dXMxn9chL56TWB31fa33J
```

![](media/spotdl-info.gif)

### **spotdl download**
Download a spotify resource to a directory. This will avoid downloading any tracks that already exist in the output directory.

When downloading an artist all its albums, singles and compilations will be downloaded but no "appears on" albums.

An example of this command was already shown above, for more details use `spotdl download --help`.

### **spotdl scan**
Scan all files in a directory and print the spotify ID and path of any tracks.

![](media/spotdl-scan.gif)

### **spotdl sync**
The `spotdl sync` commands are just helpers that maintain a manifest file that contains a list of spotify resources. These resources can then be synced using `spotdl sync download`. To add and remove resources the commands `spotdl sync add` and `spotdl sync remove` can be used. Use the `--help` flag with any of these commands to see all the possible flags.

### **spotdl cache**
spotdl attempts to cache all metadata retreived from spotify to avoid having to use the network, but sometimes it might be required to remove something from cache earlier than normal.

The cache is just a set of key-value pairs stored in the filesystem.

To list all keys use:
```bash
spotdl cache list
```

To remove one or more keys use:
```bash
spotdl cache remove <key1> <key2> ...
```

To clear the entire cache use:
```bash
spotdl cache clear
```

### **spotdl watcher**
Sometimes it might be usefull to known if a track with a given spotify id is already in your collection. It is possible to achieve this using `spotdl scan <dir>` and parsing the output, but this command can take a while to run on large collection.

To solve this problem the `spotdl watcher` commands allow you to monitor one or more directories and query for any existing tracks.

The watcher will listen on a unix socket and watch a set of directories. To start the watcher use:
```bash
spotdl watcher watch --scan <dir>
```

To query the watcher use:
```
spotdl watcher list         # List all spotify tracks (similar to scan)
spotdl watcher contains     # Check if a track is being watched
```

Here is an example systemd service that starts the watcher on login:

```conf
[Unit]
Description=spotdl watcher

[Service]
Type=simple
Environment=RUST_LOG=debug
ExecStart=/usr/bin/fish -c 'spotdl watcher watch --scan %h/music --scan-exclude %h/music/ephemeral'
RestartSec=10
Restart=always

[Install]
WantedBy=default.target
```