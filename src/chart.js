export class LineChart {
  constructor(canvasId) {
    this.canvas = document.getElementById(canvasId);
    this.ctx = this.canvas.getContext('2d');
    this.data = [];
    this.labels = [];
    
    // Använd ResizeObserver för att rita om vid ändrad fönsterstorlek
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(this.canvas.parentElement);
    this.resize();
  }

  resize() {
    const rect = this.canvas.parentElement.getBoundingClientRect();
    this.canvas.width = rect.width;
    this.canvas.height = rect.height || 200; // Säkerställ höjd
    this.draw();
  }

  addData(val, label) {
    this.data.push(val);
    this.labels.push(label);
    
    // Behåll max 50 datapunkter i vyn
    if (this.data.length > 50) {
      this.data.shift();
      this.labels.shift();
    }
    this.draw();
  }

  clear() {
    this.data = [];
    this.labels = [];
    this.draw();
  }

  draw() {
    const ctx = this.ctx;
    const w = this.canvas.width;
    const h = this.canvas.height;

    ctx.clearRect(0, 0, w, h);

    if (w === 0 || h === 0) return;

    // Marginaler
    const paddingLeft = 45;
    const paddingRight = 15;
    const paddingTop = 15;
    const paddingBottom = 20;

    const graphWidth = w - paddingLeft - paddingRight;
    const graphHeight = h - paddingTop - paddingBottom;

    // Beräkna maxvärde för Y-axeln dynamiskt
    let maxVal = 5.0; // Standard max är 5% förändring
    if (this.data.length > 0) {
      const dataMax = Math.max(...this.data) * 100;
      if (dataMax > maxVal) {
        maxVal = Math.ceil(dataMax * 1.15); // Lägg till 15% marginal
      }
    }

    // Rita rutnät och axelvärden (Y-axel)
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.04)';
    ctx.lineWidth = 1;
    ctx.fillStyle = '#64748b'; // text-muted
    ctx.font = '10px Inter';
    ctx.textAlign = 'right';

    const gridLines = 4;
    for (let i = 0; i <= gridLines; i++) {
      const yVal = (maxVal / gridLines) * i;
      const y = h - paddingBottom - (graphHeight / gridLines) * i;
      
      // Rita linje
      ctx.beginPath();
      ctx.moveTo(paddingLeft, y);
      ctx.lineTo(w - paddingRight, y);
      ctx.stroke();

      // Rita text
      ctx.fillText(`${yVal.toFixed(1)}%`, paddingLeft - 8, y + 3);
    }

    if (this.data.length < 2) {
      // Visa platshållartext om data saknas
      ctx.fillStyle = '#64748b';
      ctx.textAlign = 'center';
      ctx.font = '12px Inter';
      ctx.fillText('Väntar på skärmklippsdata...', w / 2, h / 2);
      return;
    }

    // Mappa datavärden till koordinater på canvasen
    const points = this.data.map((val, idx) => {
      const pct = val * 100;
      const x = paddingLeft + (graphWidth / (this.data.length - 1)) * idx;
      const y = h - paddingBottom - (graphHeight * (pct / maxVal));
      return { x, y };
    });

    // Rita linjen med gradient
    ctx.strokeStyle = '#6366f1'; // accent indigo
    ctx.lineWidth = 2.5;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';

    ctx.beginPath();
    ctx.moveTo(points[0].x, points[0].y);
    for (let i = 1; i < points.length; i++) {
      ctx.lineTo(points[i].x, points[i].y);
    }
    ctx.stroke();

    // Fyll ytan under grafen med en toning
    const gradient = ctx.createLinearGradient(0, paddingTop, 0, h - paddingBottom);
    gradient.addColorStop(0, 'rgba(99, 102, 241, 0.22)');
    gradient.addColorStop(1, 'rgba(99, 102, 241, 0.0)');
    ctx.fillStyle = gradient;

    ctx.beginPath();
    ctx.moveTo(points[0].x, h - paddingBottom);
    for (let i = 0; i < points.length; i++) {
      ctx.lineTo(points[i].x, points[i].y);
    }
    ctx.lineTo(points[points.length - 1].x, h - paddingBottom);
    ctx.closePath();
    ctx.fill();

    // Rita en cirkelmarkör på det senaste datavärdet
    const lastPoint = points[points.length - 1];
    ctx.fillStyle = '#6366f1';
    ctx.beginPath();
    ctx.arc(lastPoint.x, lastPoint.y, 5, 0, 2 * Math.PI);
    ctx.fill();
    
    ctx.strokeStyle = '#ffffff';
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(lastPoint.x, lastPoint.y, 5, 0, 2 * Math.PI);
    ctx.stroke();
  }
}
