#!/usr/bin/env python3

import time

import matplotlib.pyplot as plt
import pandas as pd

if __name__ == '__main__':

    df = pd.read_csv("result.csv")
    df["time"] = pd.to_datetime(df["time"], unit="us")

    for col in df.columns:
        if col != "time":
            plt.plot(df["time"], df[col], label=col)
    plt.legend()
    plt.savefig("result.png")
