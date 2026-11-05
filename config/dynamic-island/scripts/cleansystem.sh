#!/bin/bash

xhost si:localuser:root
sudo bleachbit -c --preset
xhost -si:localuser:root
bleachbit -c --preset
sudo apt clean
sudo apt autoremove --purge
bash
