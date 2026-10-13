import { formatHours, hoursOnLocalClock } from './format.js';

/**
 * When the listening happens: eight weeks as a grid of days, and the hours of a day as bars. Two
 * shapes side by side say what four charts of numbers used to.
 */
export default function StatsCharts({ heatmap, byHour }) {
  return (
    <div className="stats-charts">
      <div className="card">
        <div className="card-title">Eight weeks</div>
        <Heatmap days={heatmap} />
      </div>
      <div className="card">
        <div className="card-title">Through the day</div>
        <HourBars byHour={byHour} />
      </div>
    </div>
  );
}

/** One square per day, oldest first, a column per week; darker is more. Today is the last square. */
function Heatmap({ days }) {
  const peak = Math.max(1, ...days);
  const level = (value) => (value === 0 ? 0 : Math.min(4, Math.ceil((value / peak) * 4)));
  const firstDay = new Date(Date.now() - (days.length - 1) * 86400000);
  return (
    <div className="heatmap" style={{ '--weeks': Math.ceil(days.length / 7) }}>
      {days.map((value, index) => {
        const date = new Date(firstDay.getTime() + index * 86400000);
        return (
          <span
            key={index}
            className={`heat heat-${level(value)} ${index === days.length - 1 ? 'heat-today' : ''}`}
            title={`${date.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })} · ${formatHours(value)}`}
          />
        );
      })}
    </div>
  );
}

/** Bars scaled to the busiest hour, so a quiet day still shows its shape. */
function HourBars({ byHour }) {
  const { values, local } = hoursOnLocalClock(byHour);
  const peak = Math.max(1, ...values);
  const busiest = values.indexOf(Math.max(...values));
  return (
    <div className="hour-chart">
      <div className="hour-bars">
        {values.map((value, hour) => (
          <span
            key={hour}
            className={`hour-bar ${hour === busiest && value > 0 ? 'is-peak' : ''}`}
            style={{ height: `${Math.max(4, (value / peak) * 100)}%`, transitionDelay: `${hour * 15}ms` }}
            title={`${String(hour).padStart(2, '0')}:00 · ${formatHours(value)}`}
          />
        ))}
      </div>
      <div className="hour-axis">
        <span>00</span><span>06</span><span>12</span><span>18</span><span>{local ? '' : 'UTC'}</span>
      </div>
    </div>
  );
}
