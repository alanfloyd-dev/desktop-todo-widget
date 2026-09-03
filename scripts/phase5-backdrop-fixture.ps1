param(
  [string]$HandlePath
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$form = [System.Windows.Forms.Form]::new()
$form.Text = 'Alan Desktop Phase 5 Backdrop QA'
$form.WindowState = [System.Windows.Forms.FormWindowState]::Maximized
$form.BackColor = [System.Drawing.Color]::White
$form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
$form.KeyPreview = $true

$colors = @(
  [System.Drawing.Color]::FromArgb(255, 220, 64, 90),
  [System.Drawing.Color]::FromArgb(255, 36, 151, 219),
  [System.Drawing.Color]::FromArgb(255, 70, 190, 120),
  [System.Drawing.Color]::FromArgb(255, 245, 174, 47)
)
for ($index = 0; $index -lt $colors.Count; $index++) {
  $panel = [System.Windows.Forms.Panel]::new()
  $panel.BackColor = $colors[$index]
  $panel.Dock = [System.Windows.Forms.DockStyle]::Left
  $panel.Width = [Math]::Floor([System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width / 4)
  $form.Controls.Add($panel)
}

$label = [System.Windows.Forms.Label]::new()
$label.Text = 'NATIVE BACKDROP  •  RED  BLUE  GREEN  AMBER'
$label.AutoSize = $true
$label.Font = [System.Drawing.Font]::new('Segoe UI', 28, [System.Drawing.FontStyle]::Bold)
$label.ForeColor = [System.Drawing.Color]::White
$label.BackColor = [System.Drawing.Color]::FromArgb(120, 0, 0, 0)
$label.Location = [System.Drawing.Point]::new(80, 100)
$form.Controls.Add($label)
$label.BringToFront()

$form.Add_KeyDown({
  if ($_.KeyCode -eq [System.Windows.Forms.Keys]::Escape) {
    $form.Close()
  }
})
$form.Add_Shown({
  if ($HandlePath) {
    [System.IO.File]::WriteAllText($HandlePath, ('0x{0:X}' -f $form.Handle.ToInt64()))
  }
  $form.Activate()
})
[void]$form.ShowDialog()
