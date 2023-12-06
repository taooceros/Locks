using CodecZstd, DataFrames, CSVFiles, FileIO, CSV, DataFramesMeta, StatsBase

# Load the compressed data

cd("./visualization")


# Plot the data

using Makie, CairoMakie, AlgebraOfGraphics
using AlgebraOfGraphics: density

CairoMakie.activate!(type="svg")

data1 = CSV.read("output/proposion_counter.csv", DataFrame)

function draw_plot(dataset, filename, link_x=false)
    plt = data(dataset) * mapping(:response_time, color=(:job_length, :is_combiner) => ((x, y) -> begin
                  string.(x, pad=6) * if ismissing(y)
                      ""
                  else
                      " - " * string.(y, pad=6)
                  end
              end), layout=(:locktype, :waiter_type) => (
                  (x, y) -> begin
                      x * if ismissing(y)
                          ""
                      else
                          " - " * y
                      end
                  end
              )) * visual(ECDFPlot, npoints=1000)

    fg = draw(plt, figure=(; size=(1600, 1200)), facet=(; linkxaxes=if link_x
        :default
    else
        :none
    end))

    display(fg)

    save("graphs/$filename.svg", fg)
end

dlock_thread_32 = @chain data1 begin
    @subset(:thread_num .== 32, :locktype .∉ Ref(["Mutex", "SpinLock", "U-SCL"]))
    @subset(:response_time .< quantile(:response_time, 0.999))
    # groupby([:locktype, :waiter_type])
    # @transform(:response_time = (:response_time ./ maximum(:response_time)))
end


thread_32 = @chain data1 begin
    @subset(:thread_num .== 32)
    # groupby([:locktype, :waiter_type])
    # @transform(:response_time = (:response_time ./ maximum(:response_time)))
end

draw_plot(dlock_thread_32, "response_time_proportion_32_dlock", true)
draw_plot(thread_32, "response_time_proportion_32_all")