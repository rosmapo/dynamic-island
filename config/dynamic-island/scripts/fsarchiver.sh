#!/bin/bash

start_time=$(date +%s)

sudo rm /media/mm/Data/Zaloha/System/Debian/debianlinux3.fsa
sudo mv /media/mm/Data/Zaloha/System/Debian/debianlinux2.fsa /media/mm/Data/Zaloha/System/Debian/debianlinux3.fsa
sudo mv /media/mm/Data/Zaloha/System/Debian/debianlinux1.fsa /media/mm/Data/Zaloha/System/Debian/debianlinux2.fsa

sudo fsarchiver savefs -A -j8 /media/mm/Data/Zaloha/System/Debian/debianlinux1.fsa /dev/disk/by-uuid/BA6E-2339 /dev/disk/by-uuid/ac683399-eb02-4b20-91bb-53daac4fd4ad

end_time=$(date +%s)
echo "Time elapsed: $((end_time - start_time)) seconds"

exec bash